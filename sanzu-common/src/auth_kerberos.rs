#[cfg(windows)]
use crate::sspi;
use anyhow::{Context, Result};
#[cfg(unix)]
use libgssapi::{
    context::{ClientCtx, CtxFlags, SecurityContext, ServerCtx},
    credential::{Cred, CredUsage},
    name::Name,
    oid::{OidSet, GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE},
};

use crate::proto::*;

#[cfg(unix)]
use crate::utils::get_username_from_principal;
use crate::{tunnel, ReadWrite, Tunnel};

#[cfg(unix)]
/// Setup kerberos server context
fn setup_server_ctx() -> Result<ServerCtx> {
    let server_cred =
        Cred::acquire(None, None, CredUsage::Accept, None).context("Error in Cred::accept")?;
    debug!("Acquired server credentials: {:#?}", server_cred.info()?);
    Ok(ServerCtx::new(server_cred))
}

#[cfg(unix)]
/// Setup kerberos client context
fn setup_client_ctx(service_name: Name, desired_mechs: &OidSet) -> Result<ClientCtx> {
    let client_cred = Cred::acquire(None, None, CredUsage::Initiate, Some(desired_mechs))
        .context("Error in Cred::acquire")?;
    Ok(ClientCtx::new(
        Some(client_cred),
        service_name,
        CtxFlags::GSS_C_MUTUAL_FLAG,
        Some(&GSS_MECH_KRB5),
    ))
}

#[cfg(unix)]
/// Actually authenticate the server to the client. Using libgssapi.
pub fn do_kerberos_client_auth(
    allowed_realms: &[String],
    sock: &mut dyn ReadWrite,
) -> Result<String> {
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
    let required_flags = CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_INTEG_FLAG;
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

/// Actually authenticate the client to the given server. Using libgssapi.
#[cfg(unix)]
pub fn do_kerberos_server_auth(target_name: &str, server: &mut dyn ReadWrite) -> Result<()> {
    let desired_mechs = {
        let mut s = OidSet::new().context("Error in OidSet::new")?;
        s.add(&GSS_MECH_KRB5)
            .context("Error in add GSS_MECH_KRB5")?;
        s
    };

    let name = Name::new(target_name.as_bytes(), Some(&GSS_NT_HOSTBASED_SERVICE))
        .context("Error in new Name")?;
    let cname = name
        .canonicalize(Some(&GSS_MECH_KRB5))
        .context("Error in canonicalize")?;
    let mut client_ctx =
        setup_client_ctx(cname, &desired_mechs).context("Error in setup_client_ctx")?;
    debug!("Client ctx retrieved");
    let mut server_tok: Option<Vec<u8>> = None;
    loop {
        match client_ctx.step(server_tok.as_deref(), None)? {
            None => {
                let client_token = tunnel::EventKerberos { data: vec![] };
                send_client_msg_type!(server, client_token, Kerberos)
                    .context("Error in send EventKerberos")?;
                break;
            }
            Some(client_tok) => {
                let client_token = tunnel::EventKerberos {
                    data: client_tok.to_vec(),
                };
                send_client_msg_type!(server, client_token, Kerberos)
                    .context("Error in send EventKerberos")?;
            }
        }

        let msg = recv_client_msg_type!(server, Kerberos).context("Error in recv EventKerberos")?;
        let server_token = msg.data;
        if server_token.is_empty() {
            break;
        } else {
            server_tok = Some(server_token)
        }
    }
    info!("Security context initialized successfully");
    debug!("client ctx info: {:#?}", client_ctx.info()?);
    let flags = client_ctx.flags().context("Error in client ctx flags")?;
    let required_flags = CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_INTEG_FLAG;
    if flags & required_flags != required_flags {
        return Err(anyhow!("Kerberos flags not compliant"))
            .map_err(|err| send_client_err_event(server, err));
    }
    Ok(())
}

/// Actually authenticate the client to the given server. Using Windows Native SSPI
#[cfg(windows)]
pub fn do_kerberos_server_auth(target_name: &str, server: &mut dyn ReadWrite) -> Result<()> {
    debug!("Starting auth: target_name = {}", target_name);
    let mut ssp = sspi::SecurityPackage::new(String::from("Negotiate"), String::from(target_name));
    let mut security_status = ssp
        .acquire_credentials_handle_w()
        .context("Error in acquire_credentials_handle_w")?;
    debug!("AcquireCredentialsHandle : {:?}", security_status);

    let mut client_token = Vec::<u8>::new();

    // First security context
    security_status = ssp
        .initialize_security_context_w(&mut client_token)
        .context("Error in initialize_security_context_w")?;
    debug!("InitializeSecurityContext : {:?}", security_status);
    debug!("client_token = {:?}", &client_token);

    // Send it
    send_client_msg_type!(
        server,
        tunnel::EventKerberos { data: client_token },
        Kerberos
    )
    .context("Error in send EventKerberos")?;

    while security_status == sspi::SspiStatus::ContinueNeeded
        || security_status == sspi::SspiStatus::CompleteAndContinue
    {
        // Get response
        let response =
            recv_client_msg_type!(server, Kerberos).context("Error in recv EventKerberos")?;
        let mut server_token = response.data;
        debug!("server_token = {:?}", &server_token);
        if server_token.is_empty() {
            break;
        }

        // Step
        security_status = ssp
            .initialize_security_context_w(&mut server_token)
            .context("Error in initialize_security_context_w")?;
        debug!("security_status : {:?}", security_status);

        if security_status == sspi::SspiStatus::CompleteNeeded
            || security_status == sspi::SspiStatus::CompleteAndContinue
        {
            security_status = ssp
                .complete_auth_token(&mut server_token)
                .context("Error in complete_auth_token")?;
        };

        // Get new token
        client_token = server_token;
        debug!("client_token = {:?}", client_token);

        // Send it
        send_client_msg_type!(
            server,
            tunnel::EventKerberos { data: client_token },
            Kerberos
        )
        .context("Error in send EventKerberos")?;
    }

    info!("Security context initialized successfully");

    Ok(())
}
