use stackable_monitoring_crd::MonitoringCluster;
use stackable_operator::crd::CustomResourceExt;

fn main() -> Result<(), stackable_operator::error::Error> {
    built::write_built_file().expect("Failed to acquire build-time information");

    MonitoringCluster::write_yaml_schema("../deploy/crd/monitoringcluster.crd.yaml")?;

    Ok(())
}
