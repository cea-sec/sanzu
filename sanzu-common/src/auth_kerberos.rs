use anyhow::{Context, Result};
use libgssapi::{
    context::{CtxFlags, SecurityContext, ServerCtx},
    credential::{Cred, CredUsage},
};

use crate::proto::*;
use crate::utils::get_username_from_principal;

/// Setup kerberos server context
fn setup_server_ctx() -> Result<ServerCtx> {
    let server_cred =
        Cred::acquire(None, None, CredUsage::Accept, None).context("Error in Cred::accept")?;
    debug!("Acquired server credentials: {:#?}", server_cred.info()?);
    Ok(ServerCtx::new(server_cred))
}

pub fn do_kerberos_auth(allowed_realms: &[String], sock: &mut dyn ReadWrite) -> Result<String> {
    info!("Authenticating client");
    let mut server_ctx = setup_server_ctx()
        .context("Cannot setup server credentials")
        .map_err(|err| send_server_err_event(sock, err))?;
    loop {
        let msg = recv_client_msg_type!(sock, Kerberos).context("Error in recv EventKerberos")?;
        let client_token = msg.data;
        if client_token.is_empty() {
            break;
        }
        match server_ctx
            .step(&client_token)
            .context("Error in server step")
            .map_err(|err| send_server_err_event(sock, err))?
        {
            None => {
                break;
            }
            Some(tok) => {
                let server_token = tunnel::EventKerberos { data: tok.to_vec() };
                send_server_msg_type!(sock, server_token, Kerberos)?;
            }
        }
    }
    info!("security context initialized successfully");

    let ctx_info = server_ctx.info().context("Error in server_ctx info")?;
    debug!("server ctx info: {:#?}", ctx_info);
    let flags = server_ctx
        .flags()
        .context("Error in server ctx")
        .map_err(|err| send_server_err_event(sock, err))?;
    let required_flags =
        CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_CONF_FLAG | CtxFlags::GSS_C_INTEG_FLAG;
    if flags & required_flags != required_flags {
        return Err(anyhow!("Kerberos flags not compliant"))
            .map_err(|err| send_server_err_event(sock, err));
    }

    // Check realm
    let username = get_username_from_principal(&ctx_info.source_name.to_string(), allowed_realms)
        .context("Principal doesnt match realm pattern")
        .map_err(|err| send_server_err_event(sock, err))?;

    debug!("Auth kerberos ok for client {:?}", username);
    Ok(username)
}
