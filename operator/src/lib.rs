mod error;
mod prometheus;

use crate::error::Error;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{ConfigMap, EnvVar, Pod};
use kube::api::{ListParams, ResourceExt};
use kube::Api;
use serde_json::json;
use tracing::{debug, error, info, trace, warn};

use crate::prometheus::{
    ConfigManager, FederationTemplateDataBuilder, PodAggregatorTemplateDataBuilder,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use product_config::types::PropertyNameKind;
use product_config::ProductConfigManager;
use stackable_monitoring_crd::{
    MonitoringCluster, MonitoringClusterSpec, MonitoringClusterStatus, MonitoringRole,
    MonitoringVersion, APP_NAME, NODE_METRICS_PORT, PROMETHEUS_CONFIG_YAML, PROM_WEB_UI_PORT,
};
use stackable_operator::builder::{ContainerBuilder, ObjectMetaBuilder, PodBuilder};
use stackable_operator::client::Client;
use stackable_operator::conditions::ConditionStatus;
use stackable_operator::controller::Controller;
use stackable_operator::controller::{ControllerStrategy, ReconciliationState};
use stackable_operator::error::OperatorResult;
use stackable_operator::k8s_utils;
use stackable_operator::labels;
use stackable_operator::labels::{
    build_common_labels_for_all_managed_resources, get_recommended_labels,
};
use stackable_operator::product_config_utils::{
    config_for_role_and_group, transform_all_roles_to_config, validate_all_roles_and_groups_config,
    ValidatedRoleConfigByPropertyKind,
};
use stackable_operator::reconcile::{
    ContinuationStrategy, ReconcileFunctionAction, ReconcileResult, ReconciliationContext,
};
use stackable_operator::role_utils;
use stackable_operator::role_utils::{
    get_role_and_group_labels, list_eligible_nodes_for_role_and_group, EligibleNodesForRoleAndGroup,
};
use stackable_operator::{configmap, name_utils};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use strum::IntoEnumIterator;

const FINALIZER_NAME: &str = "monitoring.stackable.tech/cleanup";
const CONFIG_MAP_TYPE_CONFIG: &str = "config";

type MonitoringReconcileResult = ReconcileResult<error::Error>;

struct MonitoringState {
    context: ReconciliationContext<MonitoringCluster>,
    existing_pods: Vec<Pod>,
    eligible_nodes: EligibleNodesForRoleAndGroup,
    validated_role_config: ValidatedRoleConfigByPropertyKind,
}

impl MonitoringState {
    async fn set_upgrading_condition(
        &self,
        conditions: &[Condition],
        message: &str,
        reason: &str,
        status: ConditionStatus,
    ) -> OperatorResult<MonitoringCluster> {
        let resource = self
            .context
            .build_and_set_condition(
                Some(conditions),
                message.to_string(),
                reason.to_string(),
                status,
                "Upgrading".to_string(),
            )
            .await?;

        Ok(resource)
    }

    async fn set_current_version(
        &self,
        version: Option<&MonitoringVersion>,
    ) -> OperatorResult<MonitoringCluster> {
        let resource = self
            .context
            .client
            .merge_patch_status(
                &self.context.resource,
                &json!({ "currentVersion": version }),
            )
            .await?;

        Ok(resource)
    }

    async fn set_target_version(
        &self,
        version: Option<&MonitoringVersion>,
    ) -> OperatorResult<MonitoringCluster> {
        let resource = self
            .context
            .client
            .merge_patch_status(&self.context.resource, &json!({ "targetVersion": version }))
            .await?;

        Ok(resource)
    }

    /// Required labels for pods. Pods without any of these will deleted and/or replaced.
    // TODO: Now we create this every reconcile run, should be created once and reused.
    pub fn get_required_labels(&self) -> BTreeMap<String, Option<Vec<String>>> {
        let roles = MonitoringRole::iter()
            .map(|role| role.to_string())
            .collect::<Vec<_>>();
        let mut mandatory_labels = BTreeMap::new();

        mandatory_labels.insert(labels::APP_COMPONENT_LABEL.to_string(), Some(roles));
        mandatory_labels.insert(
            labels::APP_INSTANCE_LABEL.to_string(),
            Some(vec![self.context.name()]),
        );
        mandatory_labels.insert(
            labels::APP_VERSION_LABEL.to_string(),
            Some(vec![self.context.resource.spec.version.to_string()]),
        );

        mandatory_labels
    }

    /// Will initialize the status object if it's never been set.
    async fn init_status(&mut self) -> MonitoringReconcileResult {
        // We'll begin by setting an empty status here because later in this method we might
        // update its conditions. To avoid any issues we'll just create it once here.
        if self.context.resource.status.is_none() {
            let status = MonitoringClusterStatus::default();
            self.context
                .client
                .merge_patch_status(&self.context.resource, &status)
                .await?;
            self.context.resource.status = Some(status);
        }

        // This should always return either the existing one or the one we just created above.
        let status = self.context.resource.status.take().unwrap_or_default();
        let spec_version = self.context.resource.spec.version.clone();

        match (&status.current_version, &status.target_version) {
            (None, None) => {
                // No current_version and no target_version must be initial installation.
                // We'll set the Upgrading condition and the target_version to the version from spec.
                info!(
                    "Initial installation, now moving towards version [{}]",
                    self.context.resource.spec.version
                );
                self.context.resource.status = self
                    .set_upgrading_condition(
                        &status.conditions,
                        &format!("Initial installation to version [{:?}]", spec_version),
                        "InitialInstallation",
                        ConditionStatus::True,
                    )
                    .await?
                    .status;
                self.context.resource.status =
                    self.set_target_version(Some(&spec_version)).await?.status;
            }
            (None, Some(target_version)) => {
                // No current_version but a target_version means we're still doing the initial
                // installation. Will continue working towards that goal even if another version
                // was set in the meantime.
                debug!(
                    "Initial installation, still moving towards version [{}]",
                    target_version
                );
                if &spec_version != target_version {
                    info!("A new target version ([{}]) was requested while we still do the initial installation to [{}], finishing running upgrade first", spec_version, target_version)
                }
                // We do this here to update the observedGeneration if needed
                self.context.resource.status = self
                    .set_upgrading_condition(
                        &status.conditions,
                        &format!("Initial installation to version [{:?}]", target_version),
                        "InitialInstallation",
                        ConditionStatus::True,
                    )
                    .await?
                    .status;
            }
            (Some(current_version), None) => {
                // We are at a stable version but have no target_version set.
                // This will be the normal state.
                // We'll check if there is a different version in spec and if it is will
                // set it in target_version, but only if it's actually a compatible upgrade.
                if current_version != &spec_version {
                    if current_version.is_valid_upgrade(&spec_version).unwrap() {
                        let new_version = spec_version;
                        let message = format!(
                            "Upgrading from [{:?}] to [{:?}]",
                            current_version, &new_version
                        );
                        info!("{}", message);
                        self.context.resource.status =
                            self.set_target_version(Some(&new_version)).await?.status;
                        self.context.resource.status = self
                            .set_upgrading_condition(
                                &status.conditions,
                                &message,
                                "Upgrading",
                                ConditionStatus::True,
                            )
                            .await?
                            .status;
                    } else {
                        // TODO: This should be caught by an validating admission webhook
                        warn!("Upgrade from [{}] to [{}] not possible but requested in spec: Ignoring, will continue reconcile as if the invalid version weren't set", current_version, spec_version);
                    }
                } else {
                    let message = format!(
                        "No upgrade required [{:?}] is still the current_version",
                        current_version
                    );
                    trace!("{}", message);
                    self.context.resource.status = self
                        .set_upgrading_condition(
                            &status.conditions,
                            &message,
                            "",
                            ConditionStatus::False,
                        )
                        .await?
                        .status;
                }
            }
            (Some(current_version), Some(target_version)) => {
                // current_version and target_version are set means we're still in the process
                // of upgrading. We'll only do some logging and checks and will update
                // the condition so observedGeneration can be updated.
                debug!(
                    "Still upgrading from [{}] to [{}]",
                    current_version, target_version
                );
                if &self.context.resource.spec.version != target_version {
                    info!("A new target version was requested while we still upgrade from [{}] to [{}], finishing running upgrade first", current_version, target_version)
                }
                let message = format!(
                    "Upgrading from [{:?}] to [{:?}]",
                    current_version, target_version
                );

                self.context.resource.status = self
                    .set_upgrading_condition(
                        &status.conditions,
                        &message,
                        "",
                        ConditionStatus::False,
                    )
                    .await?
                    .status;
            }
        }

        Ok(ReconcileFunctionAction::Continue)
    }

    pub async fn create_missing_pods(&mut self) -> MonitoringReconcileResult {
        trace!("Starting `create_missing_pods`");

        // The iteration happens in two stages here, to accommodate the way our operators think
        // about roles and role groups.
        // The hierarchy is:
        // - Roles (for monitoring there - currently - is only a single role)
        //   - Role groups for this role (user defined)
        for monitoring_role in MonitoringRole::iter() {
            if let Some(nodes_for_role) = self.eligible_nodes.get(&monitoring_role.to_string()) {
                let role_str = &monitoring_role.to_string();
                for (role_group, (nodes, replicas)) in nodes_for_role {
                    debug!(
                        "Identify missing pods for [{}] role and group [{}]",
                        monitoring_role, role_group
                    );
                    trace!(
                        "candidate_nodes[{}]: [{:?}]",
                        nodes.len(),
                        nodes
                            .iter()
                            .map(|node| node.metadata.name.as_ref().unwrap())
                            .collect::<Vec<_>>()
                    );
                    trace!(
                        "existing_pods[{}]: [{:?}]",
                        &self.existing_pods.len(),
                        &self
                            .existing_pods
                            .iter()
                            .map(|pod| pod.metadata.name.as_ref().unwrap())
                            .collect::<Vec<_>>()
                    );
                    trace!(
                        "labels: [{:?}]",
                        get_role_and_group_labels(&monitoring_role.to_string(), role_group)
                    );
                    let nodes_that_need_pods = k8s_utils::find_nodes_that_need_pods(
                        nodes,
                        &self.existing_pods,
                        &get_role_and_group_labels(&monitoring_role.to_string(), role_group),
                        *replicas,
                    );

                    for node in nodes_that_need_pods {
                        let node_name = if let Some(node_name) = &node.metadata.name {
                            node_name
                        } else {
                            warn!("No name found in metadata, this should not happen! Skipping node: [{:?}]", node);
                            continue;
                        };
                        debug!(
                            "Creating pod on node [{}] for [{}] role and group [{}]",
                            node.metadata
                                .name
                                .as_deref()
                                .unwrap_or("<no node name found>"),
                            monitoring_role,
                            role_group
                        );

                        // now we have a node that needs pods -> get validated config
                        let validated_config = config_for_role_and_group(
                            role_str,
                            role_group,
                            &self.validated_role_config,
                        )?;

                        let config_maps = self
                            .create_config_maps(
                                &monitoring_role,
                                role_group,
                                node_name,
                                validated_config,
                            )
                            .await?;

                        self.create_pod(
                            &monitoring_role,
                            role_group,
                            node_name,
                            &config_maps,
                            validated_config,
                        )
                        .await?;

                        return Ok(ReconcileFunctionAction::Requeue(Duration::from_secs(10)));
                    }
                }
            }
        }

        let status = self.context.resource.status.clone().ok_or_else(|| error::Error::ReconcileError(
            "`Prometheus status missing, this is a programming error and should never happen. Please report in our issue tracker.".to_string(),
        ))?;

        // If we reach here it means all pods must be running on target_version.
        // We can now set current_version to target_version (if target_version was set) and
        // target_version to None
        if let Some(target_version) = &status.target_version {
            self.context.resource.status = self.set_target_version(None).await?.status;
            self.context.resource.status =
                self.set_current_version(Some(target_version)).await?.status;
            self.context.resource.status = self
                .set_upgrading_condition(
                    &status.conditions,
                    &format!(
                        "No upgrade required [{:?}] is still the current_version",
                        target_version
                    ),
                    "",
                    ConditionStatus::False,
                )
                .await?
                .status;
        }

        Ok(ReconcileFunctionAction::Continue)
    }

    /// Creates the config maps required for a monitoring instance (or role, role_group combination):
    /// * 'prometheus.yaml'
    ///
    /// The 'prometheus.yaml' properties are read from the product_config.
    ///
    /// Labels are automatically adapted from the `recommended_labels` with a type (config for
    /// 'prometheus.yaml'). Names are generated via `name_utils::build_resource_name`.
    ///
    /// Returns a map with a 'type' identifier (e.g. config) as key and the corresponding
    /// ConfigMap as value. This is required to set the volume mounts in the pod later on.
    ///
    /// # Arguments
    ///
    /// - `role` - The monitoring role.
    /// - `group` - The role group.
    /// - `node_name` - The node name for this instance.
    /// - `validated_config` - The validated product config.
    ///
    async fn create_config_maps(
        &self,
        role: &MonitoringRole,
        group: &str,
        node_name: &str,
        validated_config: &HashMap<PropertyNameKind, BTreeMap<String, String>>,
    ) -> Result<HashMap<&'static str, ConfigMap>, Error> {
        let mut config_maps = HashMap::new();
        let role_str = &role.to_string();

        let recommended_labels = get_recommended_labels(
            &self.context.resource,
            APP_NAME,
            &self.context.resource.spec.version.to_string(),
            role_str,
            group,
        );

        if let Some(config) =
            validated_config.get(&PropertyNameKind::File(PROMETHEUS_CONFIG_YAML.to_string()))
        {
            // enhance with config map type label
            let mut cm_properties_labels = recommended_labels;
            cm_properties_labels.insert(
                configmap::CONFIGMAP_TYPE_LABEL.to_string(),
                CONFIG_MAP_TYPE_CONFIG.to_string(),
            );

            let mut content = String::new();

            if &MonitoringRole::PodAggregator == role {
                // extract node_exporter_metrics_port from node -> group -> config
                let node_exporter_metrics_port =
                    self.context.resource.spec.node_exporter_metrics_port(group);

                content = ConfigManager::from_pod_aggregator_template(
                    PodAggregatorTemplateDataBuilder::new_with_namespace_and_node_name(
                        &self.context.client.default_namespace,
                        node_name,
                    )
                    .with_config(config)
                    .with_node_exporter(node_exporter_metrics_port)
                    .with_node_exporter_labels(
                        &self.context.name(),
                        node_name,
                        &self.context.namespace(),
                    )
                    .build(),
                )?
                .serialize()?;
            } else if &MonitoringRole::Federation == role {
                content = ConfigManager::from_federation_template(
                    FederationTemplateDataBuilder::new_with_namespace(
                        &self.context.client.default_namespace,
                    )
                    .with_config(config)
                    .build(),
                )?
                .serialize()?;
            }

            let mut cm_properties_data = BTreeMap::new();
            cm_properties_data.insert(PROMETHEUS_CONFIG_YAML.to_string(), content);

            let cm_properties_name = name_utils::build_resource_name(
                APP_NAME,
                &self.context.name(),
                role_str,
                Some(group),
                None,
                Some(CONFIG_MAP_TYPE_CONFIG),
            )?;

            let cm_config = configmap::build_config_map(
                &self.context.resource,
                &cm_properties_name,
                &self.context.namespace(),
                cm_properties_labels,
                cm_properties_data,
            )?;

            config_maps.insert(
                CONFIG_MAP_TYPE_CONFIG,
                configmap::create_config_map(&self.context.client, cm_config).await?,
            );
        }

        Ok(config_maps)
    }

    /// Creates the pod required for the monitoring instance.
    ///
    /// # Arguments
    ///
    /// - `role` - The monitoring role.
    /// - `group` - The role group.
    /// - `node_name` - The node name for this pod.
    /// - `config_maps` - The config maps and respective types required for this pod.
    /// - `validated_config` - The validated product config.
    ///
    async fn create_pod(
        &self,
        role: &MonitoringRole,
        group: &str,
        node_name: &str,
        config_maps: &HashMap<&'static str, ConfigMap>,
        validated_config: &HashMap<PropertyNameKind, BTreeMap<String, String>>,
    ) -> Result<Pod, Error> {
        let mut env_vars = vec![];
        // TODO: use product-config default ports?
        let mut scrape_port = "9090";
        let role_str = &role.to_string();

        // extract env variables
        if let Some(config) = validated_config.get(&PropertyNameKind::Env) {
            for (property_name, property_value) in config {
                if property_name.is_empty() {
                    warn!("Received empty property_name for ENV... skipping");
                    continue;
                }

                env_vars.push(EnvVar {
                    name: property_name.clone(),
                    value: Some(property_value.to_string()),
                    value_from: None,
                });
            }
        }

        // extract cli parameters
        if let Some(config) = validated_config.get(&PropertyNameKind::Cli) {
            if let Some(port) = config.get(PROM_WEB_UI_PORT) {
                scrape_port = port
            } else if let Some(port) = config.get(NODE_METRICS_PORT) {
                scrape_port = port
            }
        }

        let pod_name = name_utils::build_resource_name(
            APP_NAME,
            &self.context.name(),
            role_str,
            Some(group),
            Some(node_name),
            None,
        )?;

        let version = &self.context.resource.spec.version;
        let labels = get_recommended_labels(
            &self.context.resource,
            APP_NAME,
            &version.to_string(),
            role_str,
            group,
        );

        let cli_args = self.context.resource.spec.cli_args(role, group);

        let mut cb = ContainerBuilder::new(APP_NAME);
        cb.image(role.image(version));
        cb.command(role.command(version));
        cb.args(role.args(scrape_port, cli_args));
        cb.add_env_vars(env_vars);
        cb.add_container_ports(role.container_ports(scrape_port)?);

        // One mount for the config directory
        // TODO: add data volume
        if role != &MonitoringRole::NodeExporter {
            if let Some(config_map_data) = config_maps.get(CONFIG_MAP_TYPE_CONFIG) {
                if let Some(name) = config_map_data.metadata.name.as_ref() {
                    cb.add_configmapvolume(name, "conf".to_string());
                } else {
                    return Err(error::Error::MissingConfigMapNameError {
                        cm_type: CONFIG_MAP_TYPE_CONFIG,
                    });
                }
            } else {
                return Err(error::Error::MissingConfigMapError {
                    cm_type: CONFIG_MAP_TYPE_CONFIG,
                    pod_name,
                });
            }
        }

        let pod = PodBuilder::new()
            .metadata(
                ObjectMetaBuilder::new()
                    .generate_name(pod_name)
                    .namespace(&self.context.client.default_namespace)
                    .with_labels(labels)
                    .ownerreference_from_resource(&self.context.resource, Some(true), Some(true))?
                    .build()?,
            )
            .add_stackable_agent_tolerations()
            .add_container(cb.build())
            .node_name(node_name)
            .build()?;

        Ok(self.context.client.create(&pod).await?)
    }

    async fn delete_all_pods(&self) -> OperatorResult<ReconcileFunctionAction> {
        for pod in &self.existing_pods {
            self.context.client.delete(pod).await?;
        }
        Ok(ReconcileFunctionAction::Done)
    }
}

impl ReconciliationState for MonitoringState {
    type Error = error::Error;

    fn reconcile(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ReconcileFunctionAction, Self::Error>> + Send + '_>>
    {
        info!("========================= Starting reconciliation =========================");

        Box::pin(async move {
            self.init_status()
                .await?
                .then(self.context.handle_deletion(
                    Box::pin(self.delete_all_pods()),
                    FINALIZER_NAME,
                    true,
                ))
                .await?
                .then(self.context.delete_illegal_pods(
                    self.existing_pods.as_slice(),
                    &self.get_required_labels(),
                    ContinuationStrategy::OneRequeue,
                ))
                .await?
                .then(
                    self.context
                        .wait_for_terminating_pods(self.existing_pods.as_slice()),
                )
                .await?
                .then(
                    self.context
                        .wait_for_running_and_ready_pods(&self.existing_pods),
                )
                .await?
                .then(self.context.delete_excess_pods(
                    list_eligible_nodes_for_role_and_group(&self.eligible_nodes).as_slice(),
                    &self.existing_pods,
                    ContinuationStrategy::OneRequeue,
                ))
                .await?
                .then(self.create_missing_pods())
                .await
        })
    }
}

struct MonitoringStrategy {
    config: Arc<ProductConfigManager>,
}

impl MonitoringStrategy {
    pub fn new(config: ProductConfigManager) -> MonitoringStrategy {
        MonitoringStrategy {
            config: Arc::new(config),
        }
    }
}

#[async_trait]
impl ControllerStrategy for MonitoringStrategy {
    type Item = MonitoringCluster;
    type State = MonitoringState;
    type Error = Error;

    /// Init the Monitoring state. Store all available pods owned by this cluster for later processing.
    /// Retrieve nodes that fit selectors and store them for later processing:
    /// MonitoringRole (we only have 'server') -> role group -> list of nodes.
    async fn init_reconcile_state(
        &self,
        context: ReconciliationContext<Self::Item>,
    ) -> Result<Self::State, Self::Error> {
        let existing_pods = context
            .list_owned(build_common_labels_for_all_managed_resources(
                APP_NAME,
                &context.resource.name(),
            ))
            .await?;
        trace!(
            "{}: Found [{}] pods",
            context.log_name(),
            existing_pods.len()
        );

        let spec: MonitoringClusterSpec = context.resource.spec.clone();

        let mut eligible_nodes = HashMap::new();

        eligible_nodes.insert(
            MonitoringRole::PodAggregator.to_string(),
            role_utils::find_nodes_that_fit_selectors(&context.client, None, &spec.pod_aggregator)
                .await?,
        );

        if let Some(node) = &spec.node_exporter {
            eligible_nodes.insert(
                MonitoringRole::NodeExporter.to_string(),
                role_utils::find_nodes_that_fit_selectors(&context.client, None, node).await?,
            );
        }

        if let Some(federation) = &spec.federation {
            eligible_nodes.insert(
                MonitoringRole::Federation.to_string(),
                role_utils::find_nodes_that_fit_selectors(&context.client, None, federation)
                    .await?,
            );
        }

        let mut roles = HashMap::new();
        roles.insert(
            MonitoringRole::PodAggregator.to_string(),
            (
                vec![
                    PropertyNameKind::File(PROMETHEUS_CONFIG_YAML.to_string()),
                    PropertyNameKind::Cli,
                ],
                spec.pod_aggregator.clone().into(),
            ),
        );

        if let Some(node) = spec.node_exporter {
            roles.insert(
                MonitoringRole::NodeExporter.to_string(),
                (vec![PropertyNameKind::Cli], node.into()),
            );
        }

        if let Some(federation) = spec.federation {
            roles.insert(
                MonitoringRole::Federation.to_string(),
                (
                    vec![
                        PropertyNameKind::File(PROMETHEUS_CONFIG_YAML.to_string()),
                        PropertyNameKind::Cli,
                    ],
                    federation.into(),
                ),
            );
        }

        let role_config = transform_all_roles_to_config(&context.resource, roles);
        let validated_role_config = validate_all_roles_and_groups_config(
            &context.resource.spec.version.to_string(),
            &role_config,
            &self.config,
            false,
            false,
        )?;

        Ok(MonitoringState {
            context,
            existing_pods,
            eligible_nodes,
            validated_role_config,
        })
    }
}

/// This creates an instance of a [`Controller`] which waits for incoming events and reconciles them.
///
/// This is an async method and the returned future needs to be consumed to make progress.
pub async fn create_controller(client: Client, product_config_path: &str) -> OperatorResult<()> {
    let monitoring_api: Api<MonitoringCluster> = client.get_all_api();
    let pods_api: Api<Pod> = client.get_all_api();
    let config_maps_api: Api<ConfigMap> = client.get_all_api();

    let controller = Controller::new(monitoring_api)
        .owns(pods_api, ListParams::default())
        .owns(config_maps_api, ListParams::default());

    let product_config = ProductConfigManager::from_yaml_file(product_config_path).unwrap();

    let strategy = MonitoringStrategy::new(product_config);

    controller
        .run(client, strategy, Duration::from_secs(10))
        .await;

    Ok(())
}
