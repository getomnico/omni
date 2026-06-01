use crate::compose::Deployment;
use crate::StatusArgs;
use anyhow::Result;

pub async fn run(args: StatusArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let result = deployment.compose_output(["ps", "--format", "json"])?;
    if args.json {
        print_json_or_raw(&result.stdout)?;
    } else if result.success {
        deployment.compose_stream(["ps"])?;
    } else {
        eprintln!("{}", result.stderr.trim());
    }
    Ok(())
}

fn print_json_or_raw(stdout: &str) -> Result<()> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let values = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .collect::<Vec<_>>();
        if values.is_empty() {
            println!("[]");
        } else {
            println!("{}", serde_json::to_string_pretty(&values)?);
        }
    }
    Ok(())
}
