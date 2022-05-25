use anyhow::{Context, Result};

use crate::proto::*;
use pam::{self, Client};
use std::ffi::{CStr, CString};

struct TunnelConversation<'a> {
    user: Option<String>,
    password: Option<String>,
    conn: &'a mut dyn ReadWrite,
}

impl pam::Conversation for &mut TunnelConversation<'_> {
    fn prompt_echo(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg_string = msg.to_str().map_err(|_| ())?;

        let pam_msg = tunnel::pam_conversation::Msg::Echo(msg_string.to_string());
        let pam_echo = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(self.conn, pam_echo, Pamconversation).map_err(|_| ())?;

        let msg = recv_client_msg_type!(self.conn, Pamuser).map_err(|_| ())?;
        let out = CString::new(msg.user.to_owned()).map_err(|_| ())?;
        self.user = Some(msg.user);
        Ok(out)
    }
    fn prompt_blind(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg_string = msg.to_str().map_err(|_| ())?;

        let pam_msg = tunnel::pam_conversation::Msg::Blind(msg_string.to_string());
        let pam_echo = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(&mut self.conn, pam_echo, Pamconversation).map_err(|_| ())?;

        let msg = recv_client_msg_type!(self.conn, Pampwd).map_err(|_| ())?;
        self.password = Some(msg.password.to_owned());
        let out = CString::new(msg.password).map_err(|_| ())?;
        Ok(out)
    }
    fn info(&mut self, msg: &CStr) {
        let msg_string = msg.to_str().expect("Error in cstr convertion");

        let pam_msg = tunnel::pam_conversation::Msg::Info(msg_string.to_string());
        let pam_info = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(&mut self.conn, pam_info, Pamconversation)
            .expect("Error in info send");
    }
    fn error(&mut self, msg: &CStr) {
        let msg_string = msg.to_str().expect("Eror in cstr conversion");

        let pam_msg = tunnel::pam_conversation::Msg::Error(msg_string.to_string());
        let pam_err = tunnel::PamConversation { msg: Some(pam_msg) };
        send_server_msg_type!(&mut self.conn, pam_err, Pamconversation).expect("Error in err send");
    }
    fn username(&self) -> &str {
        match self.user {
            Some(ref user) => user,
            None => "",
        }
    }
}

fn send_user_info(conn: &mut dyn ReadWrite, msg: &str) -> Result<()> {
    let pam_msg = tunnel::pam_conversation::Msg::Info(msg.to_string());
    let pam_info = tunnel::PamConversation { msg: Some(pam_msg) };
    send_server_msg_type!(conn, pam_info, Pamconversation)?;

    Ok(())
}

pub fn do_pam_auth(conn: &mut dyn ReadWrite, pam_name: &str) -> Result<String> {
    let pam_user = recv_client_msg_type!(conn, Pamuser).context("Error in recv EventPamUser")?;
    let pam_pwd = recv_client_msg_type!(conn, Pampwd).context("Error in recv EventPamPwd")?;

    let mut user = pam_user.user;
    let mut password = pam_pwd.password;
    let mut final_user: Option<String> = None;
    for _ in 0..2 {
        let mut client = Client::with_password(pam_name)
            .context("Failed to init PAM client!")
            .map_err(|err| send_server_err_event(conn, err))?;

        client
            .conversation_mut()
            .set_credentials(user.clone(), password.clone());
        let ret = client.authenticate();
        if let Err(err) = ret {
            match err.0 {
                pam::PamReturnCode::New_Authtok_Reqd => {
                    warn!("Pam: user need pwd update {:?}", err);
                    send_user_info(conn, "User needs password update")?;

                    let mut conversation = TunnelConversation {
                        user: None,
                        password: None,
                        conn,
                    };

                    {
                        let mut client = Client::with_conversation("passwd", &mut conversation)
                            .context("Failed to init PAM client passwd!")?;
                        let ret = client
                            .change_authentication_token(pam::PamFlag::Change_Expired_AuthTok);
                        if ret.is_err() {
                            return Err(anyhow!("Pam: chauthtok err"));
                        }
                    }
                    // At this point, the user pwd has been successfully updated
                    // Update user / pwd and loop to login again
                    match (conversation.user.as_ref(), conversation.password.as_ref()) {
                        (Some(conv_user), Some(conv_pwd)) => {
                            user = conv_user.to_string();
                            password = conv_pwd.to_string();
                        }
                        _ => {
                            return Err(anyhow!("Pam: Cannot get user from conversation"));
                        }
                    }
                }
                pam::PamReturnCode::Auth_Err | pam::PamReturnCode::User_Unknown => {
                    error!("Authentication error");
                    return Err(send_server_err_event(conn, anyhow!("Authentication error")));
                }
                _ => {
                    error!("Unsupported error in authentication {:?}", err);
                    return Err(send_server_err_event(
                        conn,
                        anyhow!("Error during pam authentication"),
                    ));
                }
            }
        } else {
            client
                .open_session()
                .context("Error in open_session")
                .map_err(|err| send_server_err_event(conn, err))?;
            final_user = Some(user);
            break;
        }
    }
    let ret = final_user.is_some();
    send_user_info(conn, "Pam authentification ok")?;
    let pam_msg = tunnel::pam_conversation::Msg::End(ret);
    let pam_end = tunnel::PamConversation { msg: Some(pam_msg) };
    send_server_msg_type!(conn, pam_end, Pamconversation).expect("Error in end send");

    debug!("Pam user authenfified: {:?}", final_user);
    match final_user {
        None => Err(send_server_err_event(
            conn,
            anyhow!("Pam Authentification failed"),
        )),
        Some(username) => Ok(username),
    }
}
