pub mod error;

use error::Error;
use k8s_openapi::api::core::v1::ContainerPort;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};
use stackable_operator::builder::ContainerPortBuilder;
use stackable_operator::product_config_utils::{ConfigError, Configuration};
use stackable_operator::role_utils::{CommonConfiguration, Role, RoleGroup};
use stackable_operator::status::{Conditions, Status, Versioned};
use stackable_operator::versioning::{ProductVersion, Versioning, VersioningState};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use strum_macros::Display;
use strum_macros::EnumIter;

pub const APP_NAME: &str = "monitoring";
pub const MANAGED_BY: &str = "monitoring-operator";

// directories
pub const CONFIG_DIR: &str = "conf";
pub const DATA_DIR: &str = "data";

// pod and cluster (federation) metrics level
pub const PROM_SCRAPE_INTERVAL: &str = "scrapeInterval";
pub const PROM_SCRAPE_TIMEOUT: &str = "scrapeTimeout";
pub const PROM_EVALUATION_INTERVAL: &str = "evaluationInterval";
pub const PROM_WEB_UI_PORT: &str = "webUiPort";
pub const PROM_SCHEME: &str = "scheme";
// node metrics level
pub const NODE_METRICS_PORT: &str = "nodeMetricsPort";
pub const PROMETHEUS_CONFIG_YAML: &str = "prometheus.yaml";
// config map types
pub const CONFIG_MAP_TYPE_CONFIG: &str = "config";

// TODO: We need to validate the name of the cluster because it is used in pod and configmap names, it can't bee too long
// This probably also means we shouldn't use the node_names in the pod_name...
#[derive(Clone, CustomResource, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[kube(
    group = "monitoring.stackable.tech",
    version = "v1alpha1",
    kind = "MonitoringCluster",
    plural = "monitoringclusters",
    shortname = "mc",
    namespaced
)]
#[kube(status = "MonitoringClusterStatus")]
#[serde(rename_all = "camelCase")]
pub struct MonitoringClusterSpec {
    pub version: MonitoringVersion,
    pub pod_aggregator: Role<PodMonitoringConfig>,
    pub node_exporter: Option<Role<NodeMonitoringConfig>>,
    pub federation: Option<Role<PodMonitoringConfig>>,
}

impl Status<MonitoringClusterStatus> for MonitoringCluster {
    fn status(&self) -> &Option<MonitoringClusterStatus> {
        &self.status
    }
    fn status_mut(&mut self) -> &mut Option<MonitoringClusterStatus> {
        &mut self.status
    }
}

impl MonitoringClusterSpec {
    /// Extract the metrics_port for a given role_group and node_exporter role (MonitoringRole::Node).
    ///
    /// # Arguments
    /// * `role_group` - The role_group where to search for the metrics_port.
    pub fn node_exporter_metrics_port(&self, role_group: &str) -> Option<u16> {
        if let Some(Role { role_groups, .. }) = &self.node_exporter {
            if let Some(RoleGroup {
                config:
                    Some(CommonConfiguration {
                        config: Some(conf), ..
                    }),
                ..
            }) = role_groups.get(role_group)
            {
                return conf.metrics_port;
            }
        }
        None
    }

    /// Return command line args configured by the user in the custom resource for the given `role` and `group`.
    /// These arguments are appended to the mandatory argument list that is set by this operator. This list
    /// includes things like IP addresses and ports.
    ///
    /// # Arguments
    /// * `role` - The role where to search for command line args.
    /// * `group` - The role group where to search for command line args.
    pub fn cli_args(&self, role: &MonitoringRole, group: &str) -> Vec<String> {
        match role {
            MonitoringRole::PodAggregator => {
                if let Some(RoleGroup {
                    config:
                        Some(CommonConfiguration {
                            config: Some(conf), ..
                        }),
                    ..
                }) = &self.pod_aggregator.role_groups.get(group)
                {
                    return conf.cli_args.clone();
                }
            }
            MonitoringRole::NodeExporter => {
                if let Some(Role { role_groups, .. }) = &self.node_exporter {
                    if let Some(RoleGroup {
                        config:
                            Some(CommonConfiguration {
                                config: Some(conf), ..
                            }),
                        ..
                    }) = role_groups.get(group)
                    {
                        return conf.cli_args.clone();
                    }
                }
            }
            MonitoringRole::Federation => {
                if let Some(Role { role_groups, .. }) = &self.federation {
                    if let Some(RoleGroup {
                        config:
                            Some(CommonConfiguration {
                                config: Some(conf), ..
                            }),
                        ..
                    }) = role_groups.get(group)
                    {
                        return conf.cli_args.clone();
                    }
                }
            }
        }
        vec![]
    }
}

// TODO: These all should be "Property" Enums that can be either simple or complex where complex allows forcing/ignoring errors and/or warnings
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodMonitoringConfig {
    pub web_ui_port: Option<u16>,
    pub scrape_interval: Option<String>,
    pub scrape_timeout: Option<String>,
    pub evaluation_interval: Option<String>,
    pub scheme: Option<String>,
    pub cli_args: Vec<String>,
}

impl Configuration for PodMonitoringConfig {
    type Configurable = MonitoringCluster;

    fn compute_env(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        Ok(BTreeMap::new())
    }

    fn compute_cli(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        let mut result = BTreeMap::new();
        if let Some(web_ui_port) = self.web_ui_port {
            result.insert(PROM_WEB_UI_PORT.to_string(), Some(web_ui_port.to_string()));
        }
        Ok(result)
    }

    fn compute_files(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
        _file: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        let mut result = BTreeMap::new();
        if let Some(scrape_interval) = &self.scrape_interval {
            result.insert(
                PROM_SCRAPE_INTERVAL.to_string(),
                Some(scrape_interval.clone()),
            );
        }

        if let Some(scrape_timeout) = &self.scrape_timeout {
            result.insert(
                PROM_SCRAPE_TIMEOUT.to_string(),
                Some(scrape_timeout.clone()),
            );
        }

        if let Some(evaluation_interval) = &self.evaluation_interval {
            result.insert(
                PROM_EVALUATION_INTERVAL.to_string(),
                Some(evaluation_interval.clone()),
            );
        }

        if let Some(scheme) = &self.scheme {
            result.insert(PROM_SCHEME.to_string(), Some(scheme.clone()));
        }

        Ok(result)
    }
}

// TODO: These all should be "Property" Enums that can be either simple or complex where complex allows forcing/ignoring errors and/or warnings
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMonitoringConfig {
    pub metrics_port: Option<u16>,
    pub cli_args: Vec<String>,
}

impl Configuration for NodeMonitoringConfig {
    type Configurable = MonitoringCluster;

    fn compute_env(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        Ok(BTreeMap::new())
    }

    fn compute_cli(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        let mut result = BTreeMap::new();
        if let Some(metrics_port) = self.metrics_port {
            result.insert(
                NODE_METRICS_PORT.to_string(),
                Some(metrics_port.to_string()),
            );
        }
        Ok(result)
    }

    fn compute_files(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
        _file: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        Ok(BTreeMap::new())
    }
}

#[derive(EnumIter, Debug, Display, PartialEq, Eq, Hash)]
pub enum MonitoringRole {
    /// The (pod-level) metrics aggregator. One per node required.
    #[strum(serialize = "pod-aggregator")]
    PodAggregator,
    /// The (node-level) metrics. One per node required.
    #[strum(serialize = "node-exporter")]
    NodeExporter,
    /// The Federation metrics. One per cluster required.
    #[strum(serialize = "federation")]
    Federation,
}

impl MonitoringRole {
    pub fn image(&self, version: &MonitoringVersion) -> String {
        match self {
            MonitoringRole::PodAggregator | MonitoringRole::Federation => {
                format!("stackable/prometheus:{}", version.to_string())
            }
            MonitoringRole::NodeExporter => {
                format!("node-exporter:{}", version.node_exporter())
            }
        }
    }

    pub fn command(&self, version: &MonitoringVersion) -> Vec<String> {
        let mut command = vec![];
        match self {
            MonitoringRole::PodAggregator | MonitoringRole::Federation => command.push(format!(
                "prometheus-{}.linux-amd64/prometheus-wrapper.sh",
                version.to_string()
            )),
            MonitoringRole::NodeExporter => command.push(format!(
                "node_exporter-{}.linux-amd64/node_exporter",
                version.node_exporter()
            )),
        }
        command
    }

    pub fn args(&self, port: &str, args: Vec<String>) -> Vec<String> {
        let mut command = vec![];
        match self {
            MonitoringRole::PodAggregator | MonitoringRole::Federation => {
                command.push(format!("--web.listen-address=:{}", port));
                command.push(format!(
                    "--config.file={{{{configroot}}}}/{}/{}",
                    CONFIG_DIR, PROMETHEUS_CONFIG_YAML
                ));
                command.push("--log.level debug".to_string());
                if args.is_empty() {
                    command.push(format!(
                        "--storage.tsdb.path={{{{configroot}}}}/{}/",
                        DATA_DIR
                    ));
                } else {
                    command.extend(args);
                }
            }
            MonitoringRole::NodeExporter => {
                // TODO: Can 127.0.0.1 cause problems?
                command.push(format!("--web.listen-address=127.0.0.1:{}", port));
                command.extend(args);
            }
        }
        command
    }

    pub fn container_ports(&self, metrics_port: &str) -> Result<Vec<ContainerPort>, Error> {
        let mut container_ports = vec![];

        match self {
            MonitoringRole::PodAggregator => {
                container_ports.push(
                    ContainerPortBuilder::new(metrics_port.parse::<u16>()?)
                        .name("metrics")
                        .build(),
                );
                container_ports.push(
                    ContainerPortBuilder::new(metrics_port.parse::<u16>()?)
                        .name("http")
                        .build(),
                );
            }
            MonitoringRole::NodeExporter => container_ports.push(
                ContainerPortBuilder::new(metrics_port.parse::<u16>()?)
                    .name("metrics")
                    .build(),
            ),
            MonitoringRole::Federation => {
                container_ports.push(
                    ContainerPortBuilder::new(metrics_port.parse::<u16>()?)
                        .name("http")
                        .build(),
                );
            }
        }

        Ok(container_ports)
    }
}

#[allow(non_camel_case_types)]
#[derive(
    Clone,
    Debug,
    Deserialize,
    Eq,
    JsonSchema,
    PartialEq,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
pub enum MonitoringVersion {
    #[serde(rename = "2.28.1")]
    #[strum(serialize = "2.28.1")]
    v2_28_1,
    #[serde(rename = "2.30.0")]
    #[strum(serialize = "2.30.0")]
    v2_30_0,
}

impl MonitoringVersion {
    pub fn node_exporter(&self) -> &'static str {
        match self {
            MonitoringVersion::v2_28_1 | MonitoringVersion::v2_30_0 => "1.2.0",
        }
    }
}

impl Versioning for MonitoringVersion {
    fn versioning_state(&self, other: &Self) -> VersioningState {
        let from_version = match Version::parse(&self.to_string()) {
            Ok(v) => v,
            Err(e) => {
                return VersioningState::Invalid(format!(
                    "Could not parse [{}] to SemVer: {}",
                    self.to_string(),
                    e.to_string()
                ))
            }
        };

        let to_version = match Version::parse(&other.to_string()) {
            Ok(v) => v,
            Err(e) => {
                return VersioningState::Invalid(format!(
                    "Could not parse [{}] to SemVer: {}",
                    other.to_string(),
                    e.to_string()
                ))
            }
        };

        match to_version.cmp(&from_version) {
            Ordering::Greater => VersioningState::ValidUpgrade,
            Ordering::Less => VersioningState::ValidDowngrade,
            Ordering::Equal => VersioningState::NoOp,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringClusterStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "stackable_operator::conditions::schema")]
    pub conditions: Vec<Condition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<ProductVersion<MonitoringVersion>>,
}

impl Versioned<MonitoringVersion> for MonitoringClusterStatus {
    fn version(&self) -> &Option<ProductVersion<MonitoringVersion>> {
        &self.version
    }
    fn version_mut(&mut self) -> &mut Option<ProductVersion<MonitoringVersion>> {
        &mut self.version
    }
}

impl Conditions for MonitoringClusterStatus {
    fn conditions(&self) -> &[Condition] {
        self.conditions.as_slice()
    }
    fn conditions_mut(&mut self) -> &mut Vec<Condition> {
        &mut self.conditions
    }
}

#[cfg(test)]
mod tests {
    use crate::MonitoringVersion;
    use stackable_operator::versioning::{Versioning, VersioningState};
    use std::str::FromStr;

    #[test]
    fn test_zookeeper_version_versioning() {
        assert_eq!(
            MonitoringVersion::v2_28_1.versioning_state(&MonitoringVersion::v2_30_0),
            VersioningState::ValidUpgrade
        );
        assert_eq!(
            MonitoringVersion::v2_30_0.versioning_state(&MonitoringVersion::v2_28_1),
            VersioningState::ValidDowngrade
        );
        assert_eq!(
            MonitoringVersion::v2_28_1.versioning_state(&MonitoringVersion::v2_28_1),
            VersioningState::NoOp
        );
    }

    #[test]
    fn test_version_conversion() {
        MonitoringVersion::from_str("2.28.1").unwrap();
        MonitoringVersion::from_str("2.30.0").unwrap();
        MonitoringVersion::from_str("1.2.3").unwrap_err();
    }
}
