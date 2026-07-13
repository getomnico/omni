use crate::compose::Deployment;
use crate::LogsArgs;
use anyhow::Result;

pub async fn run(args: LogsArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let mut compose_args = vec![
        "logs".to_string(),
        "--tail".to_string(),
        args.tail.to_string(),
    ];
    if args.follow {
        compose_args.push("-f".to_string());
    }
    if let Some(service) = args.service {
        compose_args.push(service);
    }
    deployment.compose_stream(compose_args)?;
    Ok(())
}
