use crate::error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::{fs, str};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    /// The global configuration specifies parameters that are valid in all other configuration
    /// contexts. They also serve as defaults for other configuration sections.
    pub global: Global,
    /// A list of scrape configurations.
    pub scrape_configs: Vec<ScrapeJob>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Global {
    /// How frequently to scrape targets from this job.
    pub scrape_interval: Option<String>,
    /// Per-scrape timeout when scraping this job.
    pub scrape_timeout: Option<String>,
    /// How frequently to evaluate rules.
    pub evaluation_interval: Option<String>,
    /// A list of scrape configurations.
    pub scrape_configs: Option<Vec<ScrapeJob>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScrapeJob {
    /// The job name assigned to scraped metrics by default.
    pub job_name: String,
    /// How frequently to scrape targets from this job.
    pub scrape_interval: Option<String>,
    /// Per-scrape timeout when scraping this job.
    pub scrape_timeout: Option<String>,
    /// The HTTP resource path on which to fetch metrics from targets.
    pub metrics_path: Option<String>,
    /// Configures the protocol scheme used for requests.
    pub scheme: Option<String>,
    /// Sets the `Authorization` header on every scrape request with the
    /// configured username and password.
    /// password and password_file are mutually exclusive.
    pub basic_auth: Option<BasicAuth>,
    /// Sets the `Authorization` header on every scrape request with
    /// the configured credentials.
    pub authorization: Option<Authorization>,
    /// Optional OAuth 2.0 configuration.
    /// Cannot be used at the same time as basic_auth or authorization.
    pub oauth2: Option<OAuth2>,
    /// Configures the scrape request's TLS settings.
    pub tls_config: Option<TlsConfig>,
    /// List of Kubernetes service discovery configurations.
    pub kubernetes_sd_configs: Option<Vec<KubernetesSdConfig>>,
    /// List of labeled statically configured targets for this job.
    pub static_configs: Option<StaticSdConfig>,
    /// List of target relabel configurations.
    pub relabel_configs: Option<Vec<RelabelConfig>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: Option<String>,
    pub password_file: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Authorization {
    /// Sets the authentication type of the request.
    #[serde(rename = "type")]
    pub typ: String,
    /// Sets the credentials of the request. It is mutually exclusive with
    /// `credentials_file`.
    pub credentials: Option<String>,
    /// Sets the credentials of the request with the credentials read from the
    /// configured file. It is mutually exclusive with `credentials`.
    pub credentials_file: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuth2 {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// Read the client secret from a file.
    /// It is mutually exclusive with `client_secret`.
    pub client_secret_file: Option<String>,
    /// Scopes for the token request.
    pub scopes: Option<Vec<String>>,
    /// The URL to fetch the token from.
    pub token_url: String,
    /// Optional parameters to append to the token URL.
    pub endpoint_params: Option<HashMap<String, Option<String>>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TlsConfig {
    /// CA certificate to validate API server certificate with.
    pub ca_file: Option<String>,
    /// Certificate and key files for client cert authentication to the server.
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    /// ServerName extension to indicate the name of the server.
    /// https://tools.ietf.org/html/rfc4366#section-3.1
    pub server_name: Option<String>,
    /// Disable validation of the server certificate.
    pub insecure_skip_verify: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct KubernetesSdConfig {
    /// The API server addresses. If left empty, Prometheus is assumed to run inside
    /// of the cluster and will discover API servers automatically and use the pod's
    /// CA certificate and bearer token file at /var/run/secrets/kubernetes.io/serviceaccount/.
    pub api_server: Option<String>,
    /// The Kubernetes role of entities that should be discovered.
    /// One of endpoints, service, pod, node, or ingress.
    pub role: KubernetesSdConfigRole,
    /// Optional path to a kubeconfig file.
    /// Note that api_server and kube_config are mutually exclusive.
    pub kubeconfig_file: Option<String>,
    /// Optional authentication information used to authenticate to the API server.
    /// Note that `basic_auth` and `authorization` options are mutually exclusive.
    /// password and password_file are mutually exclusive.
    /// Optional HTTP basic authentication information.
    pub basic_auth: Option<BasicAuth>,
    /// Optional `Authorization` header configuration.
    pub authorization: Option<Authorization>,
    /// Optional OAuth 2.0 configuration.
    /// Cannot be used at the same time as basic_auth or authorization.
    pub oauth2: Option<OAuth2>,
    /// Optional proxy URL.
    pub proxy_url: Option<String>,
    /// Configure whether HTTP requests follow HTTP 3xx redirects.
    pub follow_redirects: Option<bool>,
    /// TLS configuration.
    pub tls_config: Option<TlsConfig>,
    /// Optional namespace discovery. If omitted, all namespaces are used.
    pub namespaces: Option<Namespace>,
    /// Optional label and field selectors to limit the discovery process to a subset of available resources.
    /// See https://kubernetes.io/docs/concepts/overview/working-with-objects/field-selectors/
    /// and https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/ to learn more about the possible
    /// filters that can be used. Endpoints role supports pod, service and endpoints selectors, other roles
    /// only support selectors matching the role itself (e.g. node role can only contain node selectors).
    /// Note: When making decision about using field/label selector make sure that this
    /// is the best approach - it will prevent Prometheus from reusing single list/watch
    /// for all scrape configs. This might result in a bigger load on the Kubernetes API,
    /// because per each selector combination there will be additional LIST/WATCH. On the other hand,
    /// if you just want to monitor small subset of pods in large cluster it's recommended to use selectors.
    /// Decision, if selectors should be used or not depends on the particular situation.
    pub selectors: Option<Vec<Selector>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Namespace {
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Selector {
    pub role: String,
    pub label: Option<String>,
    pub field: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KubernetesSdConfigRole {
    Endpoints,
    Ingress,
    Node,
    Pod,
    Service,
}

impl Default for KubernetesSdConfigRole {
    fn default() -> Self {
        Self::Pod
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StaticSdConfig {
    /// The targets specified by the static config.
    pub targets: Option<Vec<String>>,
    /// Labels assigned to all metrics scraped from the targets.
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelabelConfig {
    /// The source labels select values from existing labels. Their content is concatenated
    /// using the configured separator and matched against the configured regular expression
    /// for the replace, keep, and drop actions.
    pub source_labels: Option<Vec<String>>,
    /// Separator placed between concatenated source label values.
    pub separator: Option<String>,
    /// Label to which the resulting value is written in a replace action.
    /// It is mandatory for replace actions. Regex capture groups are available.
    pub target_label: Option<String>,
    /// Regular expression against which the extracted value is matched.
    pub regex: Option<String>,
    /// Modulus to take of the hash of the source label values.
    pub modulus: Option<usize>,
    /// Replacement value against which a regex replace is performed if the
    /// regular expression matches. Regex capture groups are available.
    pub replacement: Option<String>,
    /// Action to perform based on regex matching.
    pub action: Option<Action>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Drop targets for which regex matches the concatenated source_labels.
    Drop,
    /// Set target_label to the modulus of a hash of the concatenated source_labels.
    HashMod,
    /// Drop targets for which regex does not match the concatenated source_labels.
    Keep,
    /// Match regex against the concatenated source_labels. Then, set target_label to replacement,
    /// with match group references (${1}, ${2}, ...) in replacement substituted by their value.
    /// If regex does not match, no replacement takes place.
    Replace,
    /// Match regex against all label names. Then copy the values of the matching labels to label
    /// names given by replacement with match group references (${1}, ${2}, ...) in replacement
    /// substituted by their value.
    LabelMap,
    /// Match regex against all label names. Any label that matches will be removed from the set of labels.
    LabelDrop,
    /// Match regex against all label names. Any label that does not match will be removed from the set of labels.
    LabelKeep,
}

impl Default for Action {
    fn default() -> Self {
        Self::Replace
    }
}

pub struct ConfigManager {
    config: Config,
}

impl FromStr for ConfigManager {
    type Err = error::Error;
    /// Create a ProductConfig from a YAML string.
    ///
    /// # Arguments
    ///
    /// * `contents` - the YAML string content
    fn from_str(contents: &str) -> Result<Self, error::Error> {
        Ok(ConfigManager {
            config: serde_yaml::from_str(contents).map_err(|serde_error| {
                error::Error::YamlNotParsable {
                    content: contents.to_string(),
                    reason: serde_error.to_string(),
                }
            })?,
        })
    }
}

impl ConfigManager {
    /// Create a ProductConfig from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `file_path` - the path to the YAML file
    pub fn from_yaml_file(file_path: &str) -> Result<Self, error::Error> {
        let contents = fs::read_to_string(file_path).map_err(|_| error::Error::FileNotFound {
            file_name: file_path.to_string(),
        })?;

        Self::from_str(&contents).map_err(|serde_error| error::Error::YamlFileNotParsable {
            file: file_path.to_string(),
            reason: serde_error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nodepod_loads_correctly() {
        let manager = ConfigManager::from_yaml_file("data/test/nodepods.yaml");
        assert!(manager.is_ok())
    }
}
