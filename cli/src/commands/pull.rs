use crate::compose::Deployment;
use crate::PullArgs;
use anyhow::Result;

pub async fn run(args: PullArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let mut compose_args = vec!["pull".to_string()];
    compose_args.extend(args.services);
    deployment.compose_stream(compose_args)?;
    Ok(())
}
