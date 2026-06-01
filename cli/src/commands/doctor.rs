use crate::compose::Deployment;
use crate::diagnostics;
use crate::output;
use crate::DoctorArgs;
use anyhow::Result;

pub async fn run(args: DoctorArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let report = diagnostics::run_doctor(&deployment, args.verbose, &args.logs_since).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        output::print_doctor_report(&report);
    }
    Ok(())
}
