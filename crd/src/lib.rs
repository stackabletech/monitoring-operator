pub mod error;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use semver::{Error as SemVerError, Version};
use serde::{Deserialize, Serialize};
use stackable_operator::product_config_utils::{ConfigError, Configuration};
use stackable_operator::role_utils::{CommonConfiguration, Role, RoleGroup};
use stackable_operator::status::Conditions;
use std::collections::BTreeMap;

pub const APP_NAME: &str = "monitoring";
pub const MANAGED_BY: &str = "monitoring-operator";

// pod and cluster (federation) metrics level
pub const PROM_SCRAPE_INTERVAL: &str = "scrapeInterval";
pub const PROM_SCRAPE_TIMEOUT: &str = "scrapeTimeout";
pub const PROM_EVALUATION_INTERVAL: &str = "evaluationInterval";
pub const PROM_WEB_UI_PORT: &str = "webUiPort";
pub const PROM_SCHEME: &str = "scheme";
// node metrics level
pub const NODE_METRICS_PORT: &str = "nodeMetricsPort";

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
    pub node_pods: Role<PodMonitoringConfig>,
    pub node: Option<Role<NodeMonitoringConfig>>,
    pub federation: Option<Role<PodMonitoringConfig>>,
}

impl MonitoringClusterSpec {
    pub fn node_exporter_metrics_port(&self, group: &str) -> Option<u16> {
        if let Some(Role { role_groups, .. }) = &self.node {
            if let Some(RoleGroup {
                config:
                    Some(CommonConfiguration {
                        config: Some(conf), ..
                    }),
                ..
            }) = role_groups.get(group)
            {
                return conf.metrics_port;
            }
        }
        None
    }

    pub fn node_exporter_args(&self, group: &str) -> Vec<String> {
        if let Some(Role { role_groups, .. }) = &self.node {
            if let Some(RoleGroup {
                config:
                    Some(CommonConfiguration {
                        config: Some(conf), ..
                    }),
                ..
            }) = role_groups.get(group)
            {
                return conf.node_exporter_args.clone();
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
    // TODO: should the customer be able to set this?
    //pub node_exporter_version: String,
    pub node_exporter_args: Vec<String>,
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

impl Conditions for MonitoringCluster {
    fn conditions(&self) -> Option<&[Condition]> {
        if let Some(status) = &self.status {
            return Some(status.conditions.as_slice());
        }
        None
    }

    fn conditions_mut(&mut self) -> &mut Vec<Condition> {
        if self.status.is_none() {
            self.status = Some(MonitoringClusterStatus::default());
            return &mut self.status.as_mut().unwrap().conditions;
        }
        return &mut self.status.as_mut().unwrap().conditions;
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
}

impl MonitoringVersion {
    pub fn node_exporter(&self) -> &'static str {
        match self {
            MonitoringVersion::v2_28_1 => "1.2.0",
        }
    }
}

impl MonitoringVersion {
    pub fn is_valid_upgrade(&self, to: &Self) -> Result<bool, SemVerError> {
        let from_version = Version::parse(&self.to_string())?;
        let to_version = Version::parse(&to.to_string())?;
        Ok(to_version > from_version)
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringClusterStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<MonitoringVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_version: Option<MonitoringVersion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "stackable_operator::conditions::schema")]
    pub conditions: Vec<Condition>,
}

#[cfg(test)]
mod tests {}
