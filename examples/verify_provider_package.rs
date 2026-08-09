use std::{env, fs, path::PathBuf};

use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{ClientCapabilities, Implementation, InitializeRequest},
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo};
use editur::agent::provision::{SidecarManifest, provision_from_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let manifest = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: verify_provider_package MANIFEST ARCHIVE")?;
    let archive = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("missing archive path")?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let manifest = SidecarManifest::parse(&fs::read(manifest)?)?;
    let data = tempfile::tempdir()?;
    let installed = provision_from_bytes(&manifest, data.path(), &fs::read(archive)?)?;
    let agent = AcpAgent::new(AcpAgentConfig::new(&installed.command).args(installed.args));
    async_io::block_on(Client.builder().name("editur-package-probe").connect_with(
        agent,
        |connection: ConnectionTo<Agent>| async move {
            let initialized = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(ClientCapabilities::new())
                        .client_info(Implementation::new(
                            "editur-package-probe",
                            env!("CARGO_PKG_VERSION"),
                        )),
                )
                .block_task()
                .await?;
            if initialized.protocol_version != ProtocolVersion::V1 {
                return Err(agent_client_protocol::Error::invalid_request()
                    .data("packaged provider does not support stable ACP v1"));
            }
            Ok(())
        },
    ))?;
    println!(
        "Verified {} {} at {}",
        manifest.agent,
        installed.version,
        installed.command.display()
    );
    Ok(())
}
