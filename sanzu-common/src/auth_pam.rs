use anyhow::{Context, Result};

use crate::proto::*;
use pam::{self, Client};
use std::{
    ffi::{CStr, CString},
    sync::{Arc, Mutex},
};

struct TunnelConversation<'a> {
    user: Option<String>,
    password: Option<String>,
    conn: Arc<Mutex<&'a mut dyn ReadWrite>>,
}

impl pam::Conversation for &mut TunnelConversation<'_> {
    fn prompt_echo(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg_string = msg.to_str().map_err(|_| ())?;

        let pam_msg = tunnel::pam_conversation::Msg::Echo(msg_string.to_string());
        let pam_echo = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(*self.conn.lock().unwrap(), pam_echo, Pamconversation)
            .map_err(|_| ())?;

        let msg = recv_client_msg_type!(*self.conn.lock().unwrap(), Pamuser).map_err(|_| ())?;
        let out = CString::new(msg.user.to_owned()).map_err(|_| ())?;
        self.user = Some(msg.user);
        Ok(out)
    }
    fn prompt_blind(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg_string = msg.to_str().map_err(|_| ())?;

        let pam_msg = tunnel::pam_conversation::Msg::Blind(msg_string.to_string());
        let pam_echo = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(*self.conn.lock().unwrap(), pam_echo, Pamconversation)
            .map_err(|_| ())?;

        let msg = recv_client_msg_type!(*self.conn.lock().unwrap(), Pampwd).map_err(|_| ())?;
        self.password = Some(msg.password.to_owned());
        let out = CString::new(msg.password).map_err(|_| ())?;
        Ok(out)
    }
    fn info(&mut self, msg: &CStr) {
        let msg_string = msg.to_str().expect("Error in cstr convertion");

        let pam_msg = tunnel::pam_conversation::Msg::Info(msg_string.to_string());
        let pam_info = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(*self.conn.lock().unwrap(), pam_info, Pamconversation)
            .expect("Error in info send");
    }
    fn error(&mut self, msg: &CStr) {
        let msg_string = msg.to_str().expect("Eror in cstr conversion");

        let pam_msg = tunnel::pam_conversation::Msg::Error(msg_string.to_string());
        let pam_err = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(*self.conn.lock().unwrap(), pam_err, Pamconversation)
            .expect("Error in err send");
    }
    fn username(&self) -> &str {
        match self.user {
            Some(ref user) => user,
            None => "",
        }
    }
}

fn send_user_info(conn: Arc<Mutex<&mut dyn ReadWrite>>, msg: &str) -> Result<()> {
    let pam_msg = tunnel::pam_conversation::Msg::Info(msg.to_string());
    let pam_info = tunnel::PamConversation { msg: Some(pam_msg) };
    send_server_msg_type!(*conn.lock().unwrap(), pam_info, Pamconversation)?;

    Ok(())
}

pub fn do_pam_auth(conn: &mut dyn ReadWrite, pam_name: &str) -> Result<String> {
    let mut final_user = None;
    let conn = Arc::new(Mutex::new(conn));
    for _ in 0..3 {
        let mut conversation = TunnelConversation {
            user: None,
            password: None,
            conn: conn.clone(),
        };
        let mut client = Client::with_conversation(pam_name, &mut conversation)
            .context("Failed to init PAM client passwd!")?;
        let ret = client.authenticate();
        match ret {
            Ok(_) => {
                final_user = Some(client.get_user().context("Cannot get pam username")?);
                break;
            }
            Err(err) => {
                match err.0 {
                    pam::PamReturnCode::New_Authtok_Reqd => {
                        warn!("Pam: user need pwd update {:?}", err);
                        send_user_info(conn.clone(), "User needs password update")?;
                        let ret = client
                            .change_authentication_token(pam::PamFlag::Change_Expired_AuthTok);
                        if ret.is_err() {
                            return Err(anyhow!("Pam: chauthtok err"));
                        }
                        // At this point, the user pwd has been successfully updated
                        // loop to login again
                        send_user_info(conn.clone(), "User password updated")?;
                    }
                    pam::PamReturnCode::Auth_Err | pam::PamReturnCode::User_Unknown => {
                        error!("Authentication error");
                        return Err(send_server_err_event(
                            *conn.lock().unwrap(),
                            anyhow!("Authentication error"),
                        ));
                    }
                    _ => {
                        error!("Unsupported error in authentication {:?}", err);
                        return Err(send_server_err_event(
                            *conn.lock().unwrap(),
                            anyhow!("Error during pam authentication"),
                        ));
                    }
                }
            }
        }
    }

    let ret = final_user.is_some();
    let pam_msg = tunnel::pam_conversation::Msg::End(ret);
    let pam_end = tunnel::PamConversation { msg: Some(pam_msg) };
    send_server_msg_type!(*conn.lock().unwrap(), pam_end, Pamconversation)
        .expect("Error in end send");

    debug!("Pam user authenfified: {:?}", final_user);
    match final_user {
        None => Err(send_server_err_event(
            *conn.lock().unwrap(),
            anyhow!("Pam Authentification failed"),
        )),
        Some(username) => Ok(username),
    }
}
