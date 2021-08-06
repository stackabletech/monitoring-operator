use crate::error;
use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use serde_with_macros::skip_serializing_none;
use stackable_monitoring_crd::{
    PROM_EVALUATION_INTERVAL, PROM_SCHEME, PROM_SCRAPE_INTERVAL, PROM_SCRAPE_TIMEOUT,
};
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::fs;
use std::str;
use std::str::FromStr;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    /// The global configuration specifies parameters that are valid in all other configuration
    /// contexts. They also serve as defaults for other configuration sections.
    pub global: Global,
    /// A list of scrape configurations.
    pub scrape_configs: Vec<ScrapeJob>,
}

#[skip_serializing_none]
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

#[skip_serializing_none]
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
    pub static_configs: Option<Vec<StaticSdConfig>>,
    /// List of target relabel configurations.
    pub relabel_configs: Option<Vec<RelabelConfig>>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: Option<String>,
    pub password_file: Option<String>,
}

#[skip_serializing_none]
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

#[skip_serializing_none]
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

#[skip_serializing_none]
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

#[skip_serializing_none]
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

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Selector {
    pub role: KubernetesSdConfigRole,
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

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StaticSdConfig {
    /// The targets specified by the static config.
    pub targets: Option<Vec<String>>,
    /// Labels assigned to all metrics scraped from the targets.
    pub labels: Option<HashMap<String, String>>,
}

#[skip_serializing_none]
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

static NODEPODS_TEMPLATE : &str = "
global:
  evaluation_interval: {{global_evaluation_interval}}
scrape_configs:
  - job_name: nodepods
    scrape_interval: {{scrape_configs_k8s_scrape_interval}}
    scrape_timeout: {{scrape_configs_k8s_scrape_timeout}}
    scheme: {{scrape_configs_k8s_scheme}}
    kubernetes_sd_configs:
      - role: pod
        kubeconfig_file: KUBECONFIG
        namespaces:
          names:
            - {{scrape_configs_k8s_namespace}}
        selectors:
          - role: pod
            field: spec.nodeName={{scrape_configs_k8s_selector_node_name}}
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_annotation_monitoring_stackable_tech_should_be_scraped]
        regex: true
        action: keep
      - source_labels: [__meta_kubernetes_pod_container_port_name]
        action: keep
        regex: metrics
      - source_labels: [__meta_kubernetes_pod_label_app_kubernetes_io_name]
        regex: monitoring
        action: drop
      - source_labels: [__address__, __meta_kubernetes_pod_container_port_number]
        action: replace
        regex: ([^:]+)(?::\\d+)?;(\\d+)
        replacement: $1:$2
        target_label: __address__
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
        target_label: kubernetes_pod_controller_name
{{#if with_node_scraper}}
  - job_name: node
    scrape_interval: {{scrape_configs_k8s_scrape_interval}}
    static_configs:
      - targets:
        - localhost:{{node_exporter_metrics_port}}
        labels:
          node_id: this_server
{{/if}}
 ";

/// A serializer/deserializer/builder for Prometheus configuration
impl ConfigManager {
    /// Create a ProductConfig from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `file_path` - the path to the YAML file
    #[cfg(test)]
    pub fn from_yaml_file(file_path: &str) -> Result<Self, error::Error> {
        let contents = fs::read_to_string(file_path).map_err(|_| error::Error::FileNotFound {
            file_name: file_path.to_string(),
        })?;

        Self::from_str(&contents).map_err(|serde_error| error::Error::YamlFileNotParsable {
            file: file_path.to_string(),
            reason: serde_error.to_string(),
        })
    }

    /// Builds Prometheus configuration from a template (`NODEPODS_TEMPLATE`)
    ///
    /// The value 'KUBECONFIG' of the field kubeconfig_file (in kubernetes_sd_configs) will be replaced
    /// by the prometheus-wrapper.sh script with the content of the 'KUBECONFIG' environment variable
    /// set by the agent.
    ///
    /// The relabel config checks for "monitoring.stackable.tech/scrape=true" and requires a container
    /// port (to be scraped) and a container port name ("metrics"). This is required for the Service
    /// Discovery. Additionally we relabel all existing pod labels and expose the host ip, pod ip,
    /// controller kind and controller name.
    ///
    /// # Arguments
    /// * `tdata` - template variables used to render the source template.
    ///
    pub fn from_nodepods_template(
        tdata: &NodepodsTemplateDataBuilder,
    ) -> Result<Self, error::Error> {
        let tengine = Handlebars::new();
        match tengine.render_template(NODEPODS_TEMPLATE, tdata) {
            Ok(contents) => Self::from_str(contents.as_str()),
            Err(e) => Err(error::Error::YamlNotParsable {
                content: "NODEPODS_TEMPLATE".to_string(),
                reason: e.desc,
            }),
        }
    }

    /// Serialize Prometheus configuration to a Yaml string.
    pub fn serialize(&self) -> Result<String, error::Error> {
        serde_yaml::to_string(&self.config).map_err(|serde_error| {
            error::Error::PrometheusConfigCannotBeSerialized {
                reason: format!("{}", serde_error),
            }
        })
    }
}

/// A builder for template variables needed to render the `NODEPODS_TEMPLATE`
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodepodsTemplateDataBuilder {
    global_evaluation_interval: String,
    scrape_configs_k8s_scrape_interval: String,
    scrape_configs_k8s_scrape_timeout: String,
    scrape_configs_k8s_scheme: String,
    scrape_configs_k8s_namespace: String,
    scrape_configs_k8s_selector_node_name: String,
    node_exporter_metrics_port: String,
    with_node_scraper: bool,
}

impl NodepodsTemplateDataBuilder {
    pub fn new_with_namespace_and_node_name(namespace: &str, name: &str) -> Self {
        NodepodsTemplateDataBuilder {
            global_evaluation_interval: "60s".to_string(),
            scrape_configs_k8s_scrape_interval: "60s".to_string(),
            scrape_configs_k8s_scrape_timeout: "30s".to_string(),
            scrape_configs_k8s_scheme: "https".to_string(),
            scrape_configs_k8s_namespace: namespace.to_string(),
            scrape_configs_k8s_selector_node_name: name.to_string(),
            node_exporter_metrics_port: "9100".to_string(),
            with_node_scraper: false,
        }
    }

    /// Enable the static scraping section of the Prometheus configuration
    /// # Arguments
    /// * metrics_port - The port used to scrape the local node exporter. Default: 9100
    pub fn with_node_exporter(&mut self, metrics_port: Option<u16>) -> &mut Self {
        if let Some(port) = metrics_port {
            self.node_exporter_metrics_port = port.to_string();
            self.with_node_scraper = true;
        }
        self
    }

    pub fn with_config(&mut self, config: &BTreeMap<String, String>) -> &mut Self {
        if let Some(value) = config.get(PROM_EVALUATION_INTERVAL) {
            self.global_evaluation_interval = value.clone();
        }
        if let Some(value) = config.get(PROM_SCRAPE_INTERVAL) {
            self.scrape_configs_k8s_scrape_interval = value.clone();
        }
        if let Some(value) = config.get(PROM_SCRAPE_TIMEOUT) {
            self.scrape_configs_k8s_scrape_timeout = value.clone();
        }
        if let Some(value) = config.get(PROM_SCHEME) {
            self.scrape_configs_k8s_scheme = value.clone();
        }
        self
    }

    pub fn build(&mut self) -> &Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nodepods_config_loads_correctly() {
        let manager = ConfigManager::from_yaml_file("data/test/nodepods.yaml");
        assert!(manager.is_ok())
    }

    #[test]
    fn test_nodepods_and_node_config_loads_correctly() {
        let manager = ConfigManager::from_yaml_file("data/test/nodepods_and_node.yaml");
        assert!(manager.is_ok())
    }

    #[test]
    fn test_nodepods_template() {
        let manager = ConfigManager::from_nodepods_template(
            &NodepodsTemplateDataBuilder::new_with_namespace_and_node_name("default", "localhost"),
        )
        .unwrap();

        assert_eq!(
            manager.config.scrape_configs[0]
                .kubernetes_sd_configs
                .as_ref()
                .unwrap()[0]
                .selectors
                .as_ref()
                .unwrap()[0]
                .field
                .as_ref()
                .unwrap()
                .as_str(),
            "spec.nodeName=localhost"
        )
    }

    #[test]
    fn test_nodepods_and_node_template() {
        let manager = ConfigManager::from_nodepods_template(
            &NodepodsTemplateDataBuilder::new_with_namespace_and_node_name("default", "localhost")
                .with_node_exporter(Some(9111)),
        )
        .unwrap();

        assert_eq!(
            manager.config.scrape_configs[1]
                .static_configs
                .as_ref()
                .unwrap()[0]
                .targets
                .as_ref()
                .unwrap()[0],
            "localhost:9111"
        )
    }
    #[test]
    fn test_serialize() {
        let manager = ConfigManager::from_nodepods_template(
            &NodepodsTemplateDataBuilder::new_with_namespace_and_node_name("default", "localhost"),
        )
        .unwrap();
        let yaml = manager.serialize();
        assert!(yaml.is_ok())
    }
}
