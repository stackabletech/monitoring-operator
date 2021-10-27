mod error;
mod prometheus;

use crate::error::Error;
use crate::prometheus::{
    ConfigManager, FederationTemplateDataBuilder, PodAggregatorTemplateDataBuilder,
};

use async_trait::async_trait;
use stackable_monitoring_crd::{
    MonitoringCluster, MonitoringClusterSpec, MonitoringRole, APP_NAME, CONFIG_DIR,
    CONFIG_MAP_TYPE_CONFIG, NODE_METRICS_PORT, PROMETHEUS_CONFIG_YAML, PROM_WEB_UI_PORT,
};
use stackable_operator::builder::{ContainerBuilder, ObjectMetaBuilder, PodBuilder};
use stackable_operator::client::Client;
use stackable_operator::controller::Controller;
use stackable_operator::controller::{ControllerStrategy, ReconciliationState};
use stackable_operator::error::OperatorResult;
use stackable_operator::identity::{
    LabeledPodIdentityFactory, NodeIdentity, PodIdentity, PodToNodeMapping,
};
use stackable_operator::k8s_openapi::api::core::v1::{ConfigMap, EnvVar, Pod};
use stackable_operator::kube::api::{ListParams, ResourceExt};
use stackable_operator::kube::Api;
use stackable_operator::labels;
use stackable_operator::labels::{
    build_common_labels_for_all_managed_resources, get_recommended_labels,
};
use stackable_operator::product_config::types::PropertyNameKind;
use stackable_operator::product_config::ProductConfigManager;
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
use stackable_operator::scheduler::{
    K8SUnboundedHistory, RoleGroupEligibleNodes, ScheduleStrategy, Scheduler, StickyScheduler,
};
use stackable_operator::status::init_status;
use stackable_operator::versioning::{finalize_versioning, init_versioning};
use stackable_operator::{configmap, name_utils};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use strum::IntoEnumIterator;
use tracing::{debug, error, info, trace, warn};

const FINALIZER_NAME: &str = "monitoring.stackable.tech/cleanup";
const ID_LABEL: &str = "monitoring.stackable.tech/id";

type MonitoringReconcileResult = ReconcileResult<error::Error>;

struct MonitoringState {
    context: ReconciliationContext<MonitoringCluster>,
    existing_pods: Vec<Pod>,
    eligible_nodes: EligibleNodesForRoleAndGroup,
    validated_role_config: ValidatedRoleConfigByPropertyKind,
}

impl MonitoringState {
    /// Required labels for pods. Pods without any of these will deleted and/or replaced.
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
        // init status with default values if not available yet.
        self.context.resource = init_status(&self.context.client, &self.context.resource).await?;

        let spec_version = self.context.resource.spec.version.clone();

        self.context.resource =
            init_versioning(&self.context.client, &self.context.resource, spec_version).await?;

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
                for (role_group, eligible_nodes) in nodes_for_role {
                    debug!(
                        "Identify missing pods for [{}] role and group [{}]",
                        monitoring_role, role_group
                    );
                    trace!(
                        "candidate_nodes[{}]: [{:?}]",
                        eligible_nodes.nodes.len(),
                        eligible_nodes
                            .nodes
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

                    let mut history = match self
                        .context
                        .resource
                        .status
                        .as_ref()
                        .and_then(|status| status.history.as_ref())
                    {
                        Some(simple_history) => {
                            // we clone here because we cannot access mut self because we need it later
                            // to create config maps and pods. The `status` history will be out of sync
                            // with the cloned `simple_history` until the next reconcile.
                            // The `status` history should not be used after this method to avoid side
                            // effects.
                            K8SUnboundedHistory::new(&self.context.client, simple_history.clone())
                        }
                        None => K8SUnboundedHistory::new(
                            &self.context.client,
                            PodToNodeMapping::default(),
                        ),
                    };

                    let mut sticky_scheduler =
                        StickyScheduler::new(&mut history, ScheduleStrategy::GroupAntiAffinity);

                    let pod_id_factory = LabeledPodIdentityFactory::new(
                        APP_NAME,
                        &self.context.name(),
                        &self.eligible_nodes,
                        ID_LABEL,
                        1,
                    );

                    let state = sticky_scheduler.schedule(
                        &pod_id_factory,
                        &RoleGroupEligibleNodes::from(&self.eligible_nodes),
                        &self.existing_pods,
                    )?;

                    let mapping = state.remaining_mapping().filter(
                        APP_NAME,
                        &self.context.name(),
                        role_str,
                        role_group,
                    );

                    if let Some((pod_id, node_id)) = mapping.iter().next() {
                        // now we have a node that needs a pod -> get validated config
                        let validated_config = config_for_role_and_group(
                            pod_id.role(),
                            pod_id.group(),
                            &self.validated_role_config,
                        )?;

                        let config_maps = self
                            .create_config_maps(&monitoring_role, pod_id, node_id, validated_config)
                            .await?;

                        self.create_pod(
                            &monitoring_role,
                            pod_id,
                            node_id,
                            &config_maps,
                            validated_config,
                        )
                        .await?;

                        history.save(&self.context.resource).await?;

                        return Ok(ReconcileFunctionAction::Requeue(Duration::from_secs(10)));
                    }
                }
            }
        }

        // If we reach here it means all pods must be running on target_version.
        // We can now set current_version to target_version (if target_version was set) and
        // target_version to None
        finalize_versioning(&self.context.client, &self.context.resource).await?;

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
    /// - `pod_id` - The pod id for which to create config maps.
    /// - `node_id` - The node id where the pod will be placed.
    /// - `role` - The monitoring role.
    /// - `validated_config` - The validated product config.
    ///
    async fn create_config_maps(
        &self,
        role: &MonitoringRole,
        pod_id: &PodIdentity,
        node_id: &NodeIdentity,
        validated_config: &HashMap<PropertyNameKind, BTreeMap<String, String>>,
    ) -> Result<HashMap<&'static str, ConfigMap>, Error> {
        let mut config_maps = HashMap::new();

        let recommended_labels = get_recommended_labels(
            &self.context.resource,
            pod_id.app(),
            &self.context.resource.spec.version.to_string(),
            pod_id.role(),
            pod_id.group(),
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
                let node_exporter_metrics_port = self
                    .context
                    .resource
                    .spec
                    .node_exporter_metrics_port(pod_id.group());

                content = ConfigManager::from_pod_aggregator_template(
                    PodAggregatorTemplateDataBuilder::new_with_namespace_and_node_name(
                        &self.context.client.default_namespace,
                        node_id.name.as_str(),
                    )
                    .with_config(config)
                    .with_node_exporter(node_exporter_metrics_port)
                    .with_node_exporter_labels(
                        pod_id.instance(),
                        node_id.name.as_str(),
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
                pod_id.app(),
                pod_id.instance(),
                pod_id.role(),
                Some(pod_id.group()),
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
        pod_id: &PodIdentity,
        node_id: &NodeIdentity,
        config_maps: &HashMap<&'static str, ConfigMap>,
        validated_config: &HashMap<PropertyNameKind, BTreeMap<String, String>>,
    ) -> Result<Pod, Error> {
        let mut env_vars = vec![];
        // TODO: use product-config default ports?
        let mut scrape_port = "9090";

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
            pod_id.app(),
            pod_id.instance(),
            pod_id.role(),
            Some(pod_id.group()),
            Some(node_id.name.as_str()),
            None,
        )?;

        let version = &self.context.resource.spec.version;
        let mut labels = get_recommended_labels(
            &self.context.resource,
            pod_id.app(),
            &version.to_string(),
            pod_id.role(),
            pod_id.group(),
        );
        labels.insert(String::from(ID_LABEL), String::from(pod_id.id()));

        let cli_args = self.context.resource.spec.cli_args(role, pod_id.group());

        let mut cb = ContainerBuilder::new(pod_id.app());
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
                    cb.add_configmapvolume(name, CONFIG_DIR.to_string());
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
            .node_name(node_id.name.as_str())
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
