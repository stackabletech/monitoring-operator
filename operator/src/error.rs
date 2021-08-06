#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot generate Prometheus configuration: {reason}")]
    PrometheusConfigCannotBeSerialized { reason: String },

    #[cfg(test)]
    #[error("File not found: {file_name}")]
    FileNotFound { file_name: String },

    #[cfg(test)]
    #[error("Could not parse yaml file - {file}: {reason}")]
    YamlFileNotParsable { file: String, reason: String },

    #[error("Could not parse yaml - {content}: {reason}")]
    YamlNotParsable { content: String, reason: String },

    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },

    #[error("Error from Operator framework: {source}")]
    OperatorError {
        #[from]
        source: stackable_operator::error::Error,
    },

    #[error("Error from serde_json: {source}")]
    SerdeError {
        #[from]
        source: serde_json::Error,
    },

    #[error("Invalid Configmap. No name found which is required to query the ConfigMap.")]
    InvalidConfigMap,

    #[error("Error during reconciliation: {0}")]
    ReconcileError(String),

    #[error("Error creating properties file")]
    PropertiesError(#[from] product_config::writer::PropertiesWriterError),

    #[error("ProductConfig Framework reported error: {source}")]
    ProductConfigError {
        #[from]
        source: product_config::error::Error,
    },

    #[error("Operator Framework reported config error: {source}")]
    OperatorConfigError {
        #[from]
        source: stackable_operator::product_config_utils::ConfigError,
    },

    #[error("CRD module reported error: {source}")]
    CrdModuleError {
        #[from]
        source: stackable_monitoring_crd::error::Error,
    },
}
