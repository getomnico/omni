use crate::compose::Deployment;
use crate::ComposeArgs;
use anyhow::Result;

pub async fn run(args: ComposeArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let compose_args = if args.args.is_empty() {
        vec!["ps".to_string()]
    } else {
        args.args
    };
    deployment.compose_stream(compose_args)?;
    Ok(())
}
