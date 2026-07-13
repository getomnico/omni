use crate::compose::Deployment;
use crate::UpArgs;
use anyhow::Result;

pub async fn run(args: UpArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let mut compose_args = vec!["up".to_string()];
    if !args.no_detach {
        compose_args.push("-d".to_string());
    }
    if !args.no_remove_orphans {
        compose_args.push("--remove-orphans".to_string());
    }
    compose_args.extend(args.services);
    deployment.compose_stream(compose_args)?;
    Ok(())
}
