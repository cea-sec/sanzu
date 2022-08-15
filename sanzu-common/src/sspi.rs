use anyhow::{Context, Result};

use winapi::{
    ctypes::c_void,
    shared::{
        ntdef::LARGE_INTEGER,
        sspi::{
            AcquireCredentialsHandleA, CompleteAuthToken, FreeContextBuffer,
            InitializeSecurityContextW, SecBuffer, SecBufferDesc, SecHandle,
            ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_INTEGRITY, ISC_REQ_MUTUAL_AUTH,
            ISC_RET_ALLOCATED_MEMORY, ISC_RET_INTEGRITY, ISC_RET_MUTUAL_AUTH, SECBUFFER_TOKEN,
            SECBUFFER_VERSION, SECPKG_CRED_OUTBOUND, SECURITY_NATIVE_DREP,
        },
        winerror::{
            HRESULT, SEC_E_OK, SEC_I_COMPLETE_AND_CONTINUE, SEC_I_COMPLETE_NEEDED,
            SEC_I_CONTINUE_NEEDED,
        },
    },
};

#[derive(Debug, PartialEq, Eq)]
pub enum SspiStatus {
    Ok,
    ContinueNeeded,
    CompleteNeeded,
    CompleteAndContinue,
    Other(String),
}

fn u32_to_result(code: u32) -> Result<SspiStatus> {
    let hresult = code as HRESULT;
    if hresult < 0 {
        return Err(anyhow!(format!("Sspi error: {:x}", hresult)));
    }
    match hresult {
        SEC_E_OK => Ok(SspiStatus::Ok),
        SEC_I_COMPLETE_AND_CONTINUE => Ok(SspiStatus::CompleteAndContinue),
        SEC_I_COMPLETE_NEEDED => Ok(SspiStatus::CompleteNeeded),
        SEC_I_CONTINUE_NEEDED => Ok(SspiStatus::ContinueNeeded),
        _ => Ok(SspiStatus::Other("Sspi unknown status".to_string())),
    }
}

fn secbuffers_to_token(secbuffers: Vec<SecBuffer>) -> Result<Vec<u8>> {
    let bufftoken = secbuffers
        .iter()
        .find(|x| x.BufferType == SECBUFFER_TOKEN as u32)
        .context("Unable to find a buffer with Token type.")?;

    let tok: Vec<u8> = unsafe {
        std::slice::from_raw_parts(bufftoken.pvBuffer as *const _, bufftoken.cbBuffer as usize)
            .to_vec()
    };
    unsafe {
        FreeContextBuffer(bufftoken.pvBuffer);
    }
    Ok(tok)
}

pub struct SecurityPackage {
    packagename: String,
    targetname: String,
    creds: SecHandle,
    ctxt: Option<SecHandle>,
}

impl SecurityPackage {
    pub fn new(packagename: String, targetname: String) -> Self {
        SecurityPackage {
            packagename,
            targetname,
            creds: SecHandle::default(),
            ctxt: None,
        }
    }

    pub fn acquire_credentials_handle_w(&mut self) -> Result<SspiStatus> {
        u32_to_result(unsafe {
            let mut _timestamp = LARGE_INTEGER::default();
            let mut packagename = self.packagename.to_owned();
            packagename.push('\x00');
            let mut packagename_vec = packagename
                .as_bytes()
                .iter()
                .map(|&byte| byte as i8)
                .collect::<Vec<i8>>();
            let ret = AcquireCredentialsHandleA(
                std::ptr::null_mut(),         // 1:pszprincipal
                packagename_vec.as_mut_ptr(), // 2:pszpackage
                SECPKG_CRED_OUTBOUND,         // 3:fcredentialuse
                std::ptr::null_mut(),         // 4:pvlogonid
                std::ptr::null_mut(),         // 5:pauthdata
                None,                         // 6:pgetkeyfn
                std::ptr::null_mut(),         // 7:pvgetkeyargument
                &mut self.creds,              // 8:phcredential
                &mut _timestamp,              // 9:ptsexpiry
            ) as u32;
            drop(packagename_vec);
            ret
        })
    }

    pub fn initialize_security_context_w(&mut self, token: &mut Vec<u8>) -> Result<SspiStatus> {
        let creds_ptr: *mut SecHandle = &mut self.creds;
        let mut targetname_utf16 = self
            .targetname
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let (old_ctxt, new_ctxt) = if let Some(ref mut ctxt) = self.ctxt {
            (ctxt as *mut _, ctxt as *mut _)
        } else {
            self.ctxt = Some(SecHandle::default());
            (std::ptr::null_mut(), self.ctxt.as_mut().unwrap() as *mut _)
        };
        let mut in_secbuffers: Vec<SecBuffer> = Vec::new();
        let mut in_secbufferdesc: SecBufferDesc;
        let in_secbufferdesc_ptr = if token.is_empty() {
            std::ptr::null_mut()
        } else {
            in_secbuffers = vec![SecBuffer {
                BufferType: SECBUFFER_TOKEN as u32,
                cbBuffer: token.len() as u32,
                pvBuffer: token.as_mut_ptr() as *mut c_void,
            }];
            in_secbufferdesc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: in_secbuffers.len() as u32,
                pBuffers: in_secbuffers.as_mut_ptr() as *mut _,
            };
            &mut in_secbufferdesc
        };
        let mut out_secbuffers = vec![SecBuffer {
            BufferType: SECBUFFER_TOKEN as u32,
            cbBuffer: 0,
            pvBuffer: std::ptr::null_mut() as *mut c_void,
        }];
        let mut out_secbufferdesc = SecBufferDesc {
            ulVersion: SECBUFFER_VERSION,
            cBuffers: out_secbuffers.len() as u32,
            pBuffers: out_secbuffers.as_mut_ptr() as *mut _,
        };
        let out_secbufferdesc_ptr = &mut out_secbufferdesc;
        let req_flags: u32 = ISC_REQ_MUTUAL_AUTH | ISC_REQ_INTEGRITY;
        let mut ret_flags: u32 = 0;

        let result = unsafe {
            let mut _timestamp = LARGE_INTEGER::default();
            InitializeSecurityContextW(
                creds_ptr,                           // 1:phcredential
                old_ctxt,                            // 2:phcontext
                targetname_utf16.as_mut_ptr(),       // 3:psztargetname
                ISC_REQ_ALLOCATE_MEMORY | req_flags, // 4:fcontextreq
                0,                                   // 5:reserved1
                SECURITY_NATIVE_DREP,                // 6:targetdatarep
                in_secbufferdesc_ptr,                // 7:pinput
                0,                                   // 8:reserved2
                new_ctxt,                            // 9:phnewcontext
                out_secbufferdesc_ptr,               // 10:poutput
                &mut ret_flags,                      // 11:pfcontextattr
                &mut _timestamp,                     // 12:ptsexpiry
            ) as u32
        };

        if (ret_flags & ISC_RET_ALLOCATED_MEMORY)
            | (ret_flags & ISC_RET_MUTUAL_AUTH)
            | (ret_flags & ISC_RET_INTEGRITY)
            == 0
        {
            warn!("Requested SSPI flags not returned back by initializeSecurityContextW.");
        }

        // Explicit drop to ensure objects live long enough after unsafe call.
        drop(targetname_utf16);
        drop(in_secbuffers);

        *token = secbuffers_to_token(out_secbuffers).context("Error in secbuffers to token")?;

        u32_to_result(result)
    }

    pub fn complete_auth_token(&mut self, token: &mut Vec<u8>) -> Result<SspiStatus> {
        let mut secbuffers = vec![SecBuffer {
            BufferType: SECBUFFER_TOKEN as u32,
            cbBuffer: token.len() as u32,
            pvBuffer: token.as_mut_ptr() as *mut c_void,
        }];
        let mut secbufferdesc = SecBufferDesc {
            ulVersion: SECBUFFER_VERSION,
            cBuffers: secbuffers.len() as u32,
            pBuffers: secbuffers.as_mut_ptr() as *mut _,
        };
        let secbufferdesc_ptr = &mut secbufferdesc;
        let ctxt = if let Some(ref mut ctxt) = self.ctxt {
            ctxt as *mut _
        } else {
            std::ptr::null_mut()
        };
        let result = unsafe { CompleteAuthToken(ctxt, secbufferdesc_ptr) as u32 };

        *token = secbuffers_to_token(secbuffers).context("Error in secbuffers to token")?;

        u32_to_result(result)
    }
}
