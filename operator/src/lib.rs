mod error;

use crate::error::Error;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{ConfigMap, EnvVar, Node, Pod};
use kube::api::{ListParams, ResourceExt};
use kube::Api;
use serde_json::json;
use tracing::{debug, error, info, trace, warn};

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::error::ErrorResponse;
use product_config::types::PropertyNameKind;
use product_config::ProductConfigManager;
use stackable_monitoring_crd::{
    MonitoringCluster, MonitoringClusterSpec, MonitoringClusterStatus, MonitoringVersion, APP_NAME,
    PROM_EVALUATION_INTERVAL, PROM_SCHEME, PROM_SCRAPE_INTERVAL, PROM_SCRAPE_TIMEOUT,
    PROM_WEB_UI_PORT,
};
use stackable_operator::builder::{
    ConfigMapBuilder, ContainerBuilder, ContainerPortBuilder, ObjectMetaBuilder, PodBuilder,
};
use stackable_operator::cli;
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
    transform_all_roles_to_config, validate_all_roles_and_groups_config,
    ValidatedRoleConfigByPropertyKind,
};
use stackable_operator::reconcile::{
    ContinuationStrategy, ReconcileFunctionAction, ReconcileResult, ReconciliationContext,
};
use stackable_operator::role_utils::{
    get_role_and_group_labels, list_eligible_nodes_for_role_and_group,
};
use stackable_operator::{pod_utils, role_utils};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;

const FINALIZER_NAME: &str = "monitoring.stackable.tech/cleanup";
const PROMETHEUS_CONFIG_YAML: &str = "prometheus.yaml";

type MonitoringReconcileResult = ReconcileResult<error::Error>;

#[derive(EnumIter, Debug, Display, PartialEq, Eq, Hash)]
pub enum MonitoringRole {
    #[strum(serialize = "server")]
    Server,
}

struct MonitoringState {
    context: ReconciliationContext<MonitoringCluster>,
    existing_pods: Vec<Pod>,
    eligible_nodes: HashMap<String, HashMap<String, (Vec<Node>, usize)>>,
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

    /// Create or update a config map.
    /// - Create if no config map of that name exists
    /// - Update if config map exists but the content differs
    /// - Do nothing if the config map exists and the content is identical
    /// - Forward any kube errors that may appear
    // TODO: move to operator-rs
    async fn create_config_map(&self, config_map: ConfigMap) -> Result<(), Error> {
        let cm_name = match config_map.metadata.name.as_deref() {
            None => return Err(Error::InvalidConfigMap),
            Some(name) => name,
        };

        match self
            .context
            .client
            .get::<ConfigMap>(cm_name, Some(&self.context.namespace()))
            .await
        {
            Ok(ConfigMap {
                data: existing_config_map_data,
                ..
            }) if existing_config_map_data == config_map.data => {
                debug!(
                    "ConfigMap [{}] already exists with identical data, skipping creation!",
                    cm_name
                );
            }
            Ok(_) => {
                debug!(
                    "ConfigMap [{}] already exists, but differs, updating it!",
                    cm_name
                );
                self.context.client.update(&config_map).await?;
            }
            Err(stackable_operator::error::Error::KubeError {
                source: kube::error::Error::Api(ErrorResponse { reason, .. }),
            }) if reason == "NotFound" => {
                debug!("Error getting ConfigMap [{}]: [{:?}]", cm_name, reason);
                self.context.client.create(&config_map).await?;
            }
            Err(e) => return Err(Error::OperatorError { source: e }),
        }

        Ok(())
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
                        let validated_config = match self
                            .validated_role_config
                            .get(&monitoring_role.to_string())
                        {
                            None => {
                                error!("Could not find combination of Role [{}] in product-config. This should not happen, please open a ticket.", monitoring_role.to_string());
                                continue;
                            }
                            Some(role_groups) => match role_groups.get(role_group) {
                                None => {
                                    error!("Could not find combination of Role [{}] and RoleGroup [{}] in product-config. This should not happen, please open a ticket.", monitoring_role.to_string(), role_group);
                                    continue;
                                }
                                Some(validated_config) => validated_config,
                            },
                        };

                        let pod_name = pod_utils::get_pod_name(
                            APP_NAME,
                            &self.context.name(),
                            role_group,
                            &monitoring_role.to_string(),
                            node_name,
                        );

                        let pod_labels = get_recommended_labels(
                            &self.context.resource,
                            APP_NAME,
                            &self.context.resource.spec.version.to_string(),
                            &monitoring_role.to_string(),
                            role_group,
                        );

                        let (pod, config_maps) = self
                            .create_pod_and_config_maps(
                                &node_name,
                                &pod_name,
                                pod_labels,
                                validated_config,
                            )
                            .await?;

                        for config_map in config_maps {
                            self.create_config_map(config_map).await?;
                        }

                        self.context.client.create(&pod).await?;

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
            self.context.resource.status = self
                .set_current_version(Some(&target_version))
                .await?
                .status;
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

    async fn delete_all_pods(&self) -> OperatorResult<ReconcileFunctionAction> {
        for pod in &self.existing_pods {
            self.context.client.delete(pod).await?;
        }
        Ok(ReconcileFunctionAction::Done)
    }

    /// This method creates a pod and required config map(s) for a certain role and role_group.
    /// The validated_config from the product-config is used to create the config map data, as
    /// well as setting the ENV variables in the containers or adapt / expand the CLI parameters.
    /// First we iterate through the validated_config and extract files (which represents one or
    /// more config map(s)), env variables for the pod containers and cli parameters for the
    /// container start command and arguments.
    async fn create_pod_and_config_maps(
        &self,
        node_name: &str,
        pod_name: &str,
        labels: BTreeMap<String, String>,
        validated_config: &HashMap<PropertyNameKind, BTreeMap<String, String>>,
    ) -> Result<(Pod, Vec<ConfigMap>), Error> {
        let mut config_maps = vec![];
        let mut env_vars = vec![];
        let mut cli_web_ui_port = "9090";

        let cm_config_name = format!("{}-config", pod_name);

        for (property_name_kind, config) in validated_config {
            match property_name_kind {
                PropertyNameKind::File(file_name) => {
                    if file_name.as_str() != PROMETHEUS_CONFIG_YAML {
                        continue;
                    }

                    let content = build_prometheus_yaml(
                        &self.context.client.default_namespace,
                        node_name,
                        config.get(PROM_SCRAPE_INTERVAL),
                        config.get(PROM_SCRAPE_TIMEOUT),
                        config.get(PROM_EVALUATION_INTERVAL),
                        config.get(PROM_SCHEME),
                    );

                    let mut cm_config_data = BTreeMap::new();
                    cm_config_data.insert(file_name.clone(), content);

                    config_maps.push(
                        ConfigMapBuilder::new()
                            .metadata(
                                ObjectMetaBuilder::new()
                                    .name(cm_config_name.clone())
                                    .ownerreference_from_resource(
                                        &self.context.resource,
                                        Some(true),
                                        Some(true),
                                    )?
                                    .namespace(&self.context.client.default_namespace)
                                    .build()?,
                            )
                            .data(cm_config_data)
                            .build()?,
                    );
                }
                PropertyNameKind::Env => {
                    for (property_name, property_value) in config {
                        if property_name.is_empty() {
                            warn!("Received empty property_name for ENV... skipping");
                            continue;
                        }

                        env_vars.push(EnvVar {
                            name: property_name.clone(),
                            value: Some(property_value.clone()),
                            value_from: None,
                        });
                    }
                }
                PropertyNameKind::Cli => {
                    if let Some(port) = config.get("webUiPort") {
                        cli_web_ui_port = port
                    }
                }
            }
        }

        let version = &self.context.resource.spec.version.to_string();

        let mut container_builder = ContainerBuilder::new("monitoring");
        container_builder.image(format!("stackable/prometheus:{}", version.to_string()));
        container_builder.command(vec![
            format!(
                "prometheus-{}.linux-amd64/prometheus-wrapper.sh",
                version.to_string()
            ),
            format!(
                "--config.file={{{{configroot}}}}/conf/{}",
                PROMETHEUS_CONFIG_YAML
            ),
            "--log.level debug".to_string(),
            // TODO: Enable admin api? Make configurable.
            "--web.enable-admin-api".to_string(),
            format!("--web.listen-address=:{}", cli_web_ui_port),
        ]);
        // One mount for the config directory, this will be relative to the extracted package
        container_builder.add_configmapvolume(cm_config_name, "conf".to_string());
        container_builder.add_env_vars(env_vars);
        // Expose the web ui port as container port
        container_builder.add_container_port(
            ContainerPortBuilder::new(cli_web_ui_port.parse::<u16>()?)
                .name(PROM_WEB_UI_PORT.to_lowercase())
                .build(),
        );

        let pod = PodBuilder::new()
            .metadata(
                ObjectMetaBuilder::new()
                    .name(pod_name)
                    .namespace(&self.context.client.default_namespace)
                    .with_labels(labels)
                    .ownerreference_from_resource(&self.context.resource, Some(true), Some(true))?
                    .build()?,
            )
            .add_stackable_agent_tolerations()
            .add_container(container_builder.build())
            .node_name(node_name)
            .build()?;

        Ok((pod, config_maps))
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
            MonitoringRole::Server.to_string(),
            role_utils::find_nodes_that_fit_selectors(&context.client, None, &spec.servers).await?,
        );

        let mut roles = HashMap::new();
        roles.insert(
            MonitoringRole::Server.to_string(),
            (
                vec![
                    PropertyNameKind::File(PROMETHEUS_CONFIG_YAML.to_string()),
                    PropertyNameKind::Cli,
                ],
                context.resource.spec.servers.clone().into(),
            ),
        );

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
pub async fn create_controller(client: Client) -> OperatorResult<()> {
    let monitoring_api: Api<MonitoringCluster> = client.get_all_api();
    let pods_api: Api<Pod> = client.get_all_api();
    let config_maps_api: Api<ConfigMap> = client.get_all_api();

    let controller = Controller::new(monitoring_api)
        .owns(pods_api, ListParams::default())
        .owns(config_maps_api, ListParams::default());

    let product_config_path = cli::product_config_path(
        "monitoring-operator",
        vec![
            "deploy/config-spec/properties.yaml",
            "/etc/stackable/monitoring-operator/config-spec/properties.yaml",
        ],
    )?;

    let product_config = ProductConfigManager::from_yaml_file(&product_config_path).unwrap();

    let strategy = MonitoringStrategy::new(product_config);

    controller
        .run(client, strategy, Duration::from_secs(10))
        .await;

    Ok(())
}

/// Builds a prometheus yaml configuration file using Kubernetes Service Discovery.
///
/// The value 'KUBECONFIG' of the field kubeconfig_file (in kubernetes_sd_configs) will be replaced
/// by the prometheus-wrapper.sh script with the content of the 'KUBECONFIG' environment variable
/// set by the agent.
///
/// The relabel config checks for "monitoring.stackable.tech/scrape=true" and requires a container
/// port (to be scraped) and a container port name ("metrics"). This is required for the Service
/// Discovery. Additionally we relabel all existing pod labels and expose the host ip, pod ip,
/// controller kind and controller name.
fn build_prometheus_yaml(
    namespace: &str,
    node_name: &str,
    scrape_interval: Option<&String>,
    scrape_timeout: Option<&String>,
    evaluation_interval: Option<&String>,
    scheme: Option<&String>,
) -> String {
    // TODO: change "should_be_scraped" to "scrape"
    format!("
global:
  evaluation_interval: {}
scrape_configs:
  - job_name: k8pods
    scrape_interval: {}
    scrape_timeout: {}
    scheme: {}
    kubernetes_sd_configs:
      - role: pod
        kubeconfig_file: KUBECONFIG
        namespaces:
          names:
            - {}
        selectors:
          - role: pod
            field: spec.nodeName={}
    relabel_configs:
      # only keep pods that have the \"monitoring.stackable.tech/should_be_scraped = true\" annotation.
      - source_labels: [__meta_kubernetes_pod_annotation_monitoring_stackable_tech_should_be_scraped]
        regex: true
        action: keep
      # do not scrape yourself
      - source_labels: [__meta_kubernetes_pod_label_app_kubernetes_io_name]
        regex: monitoring
        action: drop
      # adapt port to provided pod container port
      - source_labels: [__address__, __meta_kubernetes_pod_container_port_number]
        action: replace
        regex: ([^:]+)(?::\\d+)?;(\\d+)
        replacement: $1:$2
        target_label: __address__
      # use all provided pod labels
      - action: labelmap
        regex: __meta_kubernetes_pod_label_(.+)
      - source_labels: [__meta_kubernetes_namespace]
        action: replace
        target_label: kubernetes_namespace
      - source_labels: [__meta_kubernetes_pod_name]
        action: replace
        target_label: kubernetes_pod_name
      - source_labels: [__meta_kubernetes_pod_node_name]
        action: replace
        target_label: kubernetes_pod_node_name
      - source_labels: [__meta_kubernetes_pod_host_ip]
        action: replace
        target_label: kubernetes_pod_host_ip
      - source_labels: [__meta_kubernetes_pod_uid]
        action: replace
        target_label: kubernetes_pod_uid
      - source_labels: [__meta_kubernetes_pod_controller_kind]
        action: replace
        target_label: kubernetes_pod_controller_kind
      - source_labels: [__meta_kubernetes_pod_controller_name]
        action: replace
        target_label: kubernetes_pod_controller_name", 
        // TODO: this overwrites defaults from product config
        //   Need to prepare this file in a better way
        evaluation_interval.unwrap_or(&"10s".to_string()),
        scrape_interval.unwrap_or(&"10s".to_string()),
        scrape_timeout.unwrap_or(&"10s".to_string()),
        scheme.unwrap_or(&"http".to_string()),
        namespace,
        node_name
    )
}

#[cfg(test)]
mod tests {}
