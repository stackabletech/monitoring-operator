use clap::{crate_version, App, AppSettings, SubCommand};
use stackable_monitoring_crd::MonitoringCluster;
use stackable_operator::kube::CustomResourceExt;
use stackable_operator::{cli, logging};
use stackable_operator::{client, error};
use tracing::error;

mod built_info {
    // The file has been placed there by the build script.
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() -> Result<(), error::Error> {
    logging::initialize_logging("MONITORING_OPERATOR_LOG");

    // Handle CLI arguments
    let matches = App::new(built_info::PKG_DESCRIPTION)
        .author("Stackable GmbH - info@stackable.de")
        .about(built_info::PKG_DESCRIPTION)
        .version(crate_version!())
        .arg(cli::generate_productconfig_arg())
        .subcommand(
            SubCommand::with_name("crd")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(cli::generate_crd_subcommand::<MonitoringCluster>()),
        )
        .get_matches();

    if let ("crd", Some(subcommand)) = matches.subcommand() {
        if cli::handle_crd_subcommand::<MonitoringCluster>(subcommand)? {
            return Ok(());
        }
    }

    let paths = vec![
        "deploy/config-spec/properties.yaml",
        "/etc/stackable/monitoring-operator/config-spec/properties.yaml",
    ];
    let product_config_path = cli::handle_productconfig_arg(&matches, paths)?;

    stackable_operator::utils::print_startup_string(
        built_info::PKG_DESCRIPTION,
        built_info::PKG_VERSION,
        built_info::GIT_VERSION,
        built_info::TARGET,
        built_info::BUILT_TIME_UTC,
        built_info::RUSTC_VERSION,
    );

    let client = client::create_client(Some("monitoring.stackable.tech".to_string())).await?;

    // This will wait for (but not create) all CRDs we need.
    if let Err(error) = stackable_operator::crd::wait_until_crds_present(
        &client,
        vec![MonitoringCluster::crd_name()],
        None,
    )
    .await
    {
        error!("Required CRDs missing, aborting: {:?}", error);
        return Err(error);
    };

    stackable_monitoring_operator::create_controller(client, &product_config_path).await
}
