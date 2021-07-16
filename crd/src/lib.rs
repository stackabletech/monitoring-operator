pub mod error;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use semver::{Error as SemVerError, Version};
use serde::{Deserialize, Serialize};
use stackable_operator::product_config_utils::{ConfigError, Configuration};
use stackable_operator::role_utils::Role;
use stackable_operator::status::Conditions;
use std::collections::BTreeMap;

pub const APP_NAME: &str = "monitoring";
pub const MANAGED_BY: &str = "monitoring-operator";

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
pub struct MonitoringClusterSpec {
    pub version: MonitoringVersion,
    pub servers: Role<MonitoringConfig>,
}

// TODO: These all should be "Property" Enums that can be either simple or complex where complex allows forcing/ignoring errors and/or warnings
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringConfig {
    pub port: Option<u16>,
    pub scrape_interval: Option<usize>,
    pub evaluation_interval: Option<usize>,
}

impl Configuration for MonitoringConfig {
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
        Ok(BTreeMap::new())
    }

    fn compute_files(
        &self,
        _resource: &Self::Configurable,
        _role_name: &str,
        _file: &str,
    ) -> Result<BTreeMap<String, Option<String>>, ConfigError> {
        let temp = product_config::ser::to_hash_map(self).map_err(|err| {
            ConfigError::InvalidConfiguration {
                reason: format!("Could not deserialize config: {}", err.to_string()),
            }
        })?;
        let result: BTreeMap<String, Option<String>> =
            temp.into_iter().map(|(k, v)| (k, Some(v))).collect();
        Ok(result)
    }
}

impl Conditions for MonitoringCluster {
    fn conditions(&self) -> Option<&[Condition]> {
        if let Some(status) = &self.status {
            return Some(&status.conditions.as_slice());
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
