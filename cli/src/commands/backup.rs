use crate::compose::Deployment;
use crate::managed_files;
use crate::BackupArgs;
use anyhow::Result;

pub async fn run(args: BackupArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let backup_dir = managed_files::create_backup(&deployment.root, &[".env"])?;
    println!("Created backup at {}", backup_dir.display());
    Ok(())
}
