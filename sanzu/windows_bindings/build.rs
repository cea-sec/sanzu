fn main() {
    windows::build! {
        Windows::Win32::Foundation::{PWSTR, SEC_I_COMPLETE_AND_CONTINUE, SEC_I_COMPLETE_NEEDED, SEC_I_CONTINUE_NEEDED, SEC_E_OK},
        Windows::Win32::Security::Authentication::Identity::Core::{AcquireCredentialsHandleW, CompleteAuthToken, FreeContextBuffer, InitializeSecurityContextW, SecBuffer, SecBufferDesc, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_INTEGRITY, ISC_REQ_MUTUAL_AUTH, ISC_RET_ALLOCATED_MEMORY, ISC_RET_INTEGRITY, ISC_RET_MUTUAL_AUTH, SECBUFFER_VERSION, SECURITY_NATIVE_DREP, SECBUFFER_TOKEN},
        Windows::Win32::Security::Credentials::SecHandle,
    };
}
