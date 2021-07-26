use stackable_monitoring_crd::MonitoringCluster;
use stackable_operator::crd::CustomResourceExt;
use stackable_operator::logging;
use stackable_operator::{client, error};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), error::Error> {
    logging::initialize_logging("MONITORING_OPERATOR_LOG");

    info!("Starting Stackable Operator for Monitoring and Metrics");
    let client = client::create_client(Some("monitoring.stackable.tech".to_string())).await?;

    if let Err(error) = stackable_operator::crd::wait_until_crds_present(
        &client,
        vec![&MonitoringCluster::crd_name()],
        None,
    )
    .await
    {
        error!("Required CRDs missing, aborting: {:?}", error);
        return Err(error);
    };

    stackable_monitoring_operator::create_controller(client).await?;
    Ok(())
}
