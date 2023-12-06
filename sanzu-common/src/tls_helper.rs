pub use crate::ReadWrite;
use anyhow::{Context, Result};
use rustls::ServerConnection;
use rustls::{self, server::WebPkiClientVerifier, RootCertStore};
use rustls_pki_types::{CertificateDer, CertificateRevocationListDer, PrivateKeyDer};
use std::{
    fs,
    io::{BufReader, Read, Write},
    sync::Arc,
};
use x509_parser::extensions::{GeneralName::RFC822Name, ParsedExtension, SubjectAlternativeName};

/// Load private key from @filename
fn load_private_key(filename: &str) -> Result<PrivateKeyDer<'static>> {
    let keyfile = fs::File::open(filename).context("Cannot open private key file")?;
    let mut reader = BufReader::new(keyfile);

    loop {
        match rustls_pemfile::read_one(&mut reader).context("Cannot parse private key file")? {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => return Ok(key.into()),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => return Ok(key.into()),
            None => {
                return Err(anyhow!(
                    "No keys found in {:?} (encrypted keys not supported)",
                    filename
                ))
            }
            _ => {}
        }
    }
}

/// Load Certificates from @filename
fn load_certs(filename: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certfile = fs::File::open(filename).context("Cannot open certificate file")?;
    let mut reader = BufReader::new(certfile);
    let mut certs = vec![];
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.context("Error in parsing cert")?;
        certs.push(cert);
    }
    Ok(certs)
}

/// Load ocsp from @filename
fn load_ocsp(filename: &str) -> Result<Vec<u8>> {
    let mut ret = Vec::new();

    fs::File::open(filename)
        .context("cannot open ocsp file")?
        .read_to_end(&mut ret)
        .context("Cannot read ocsp file")?;
    Ok(ret)
}
/// Apply tls operation to socket
fn tls_transfer<T: Read + Write>(server: &mut ServerConnection, socket: &mut T) -> Result<()> {
    if server.wants_write() {
        server.write_tls(socket)?;
    }

    if server.wants_read() {
        server.read_tls(socket)?;
    }
    Ok(())
}

/// Loop until tls handshake is done
pub fn tls_do_handshake<T: Read + Write>(
    server: &mut ServerConnection,
    socket: &mut T,
) -> Result<()> {
    while server.is_handshaking() {
        tls_transfer(server, socket).context("Error in tls_transfer")?;
        server
            .process_new_packets()
            .context("Error in process_new_packets")?;
    }
    Ok(())
}

/// Make tls server config from config file
pub fn make_server_config(
    ca_file: &str,
    crl_file: Option<&str>,
    ocsp_file: Option<&str>,
    server_cert: &str,
    server_key: &str,
    auth_client: bool,
) -> Result<Arc<rustls::ServerConfig>> {
    let mut client_auth_roots = RootCertStore::empty();
    let roots = load_certs(ca_file).context("Cannot load ca certificates")?;
    for root in roots {
        client_auth_roots
            .add(root)
            .context("Cannot add root cert")?;
    }
    let certs = load_certs(server_cert).context("Cannot load server ceritifactes")?;

    let client_verifier = WebPkiClientVerifier::builder(client_auth_roots.into());
    let client_verifier = if let Some(crl_file) = crl_file {
        let mut crl_file = fs::File::open(crl_file).context("Cannot open crl file")?;
        let mut crl = Vec::default();
        crl_file
            .read_to_end(&mut crl)
            .context("Cannot read crl file")?;

        client_verifier.with_crls([CertificateRevocationListDer::from(crl)])
    } else {
        client_verifier
    };

    let client_verifier = if auth_client {
        client_verifier
    } else {
        client_verifier.allow_unauthenticated()
    };
    let client_auth = client_verifier
        .build()
        .context("Cannot build client verifier")?;

    let suites = rustls::crypto::ring::ALL_CIPHER_SUITES.to_vec();

    let versions = rustls::ALL_VERSIONS.to_vec();

    let privkey = load_private_key(server_key).context("Cannot load private key")?;

    let ocsp = if let Some(ocsp_file) = ocsp_file {
        load_ocsp(ocsp_file).context("Error in load ocsp file")?
    } else {
        vec![]
    };

    let mut config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::CryptoProvider {
            cipher_suites: suites,
            ..rustls::crypto::ring::default_provider()
        }
        .into(),
    )
    .with_protocol_versions(&versions)
    .context("Inconsistent cipher-suite/versions selected")?
    .with_client_cert_verifier(client_auth)
    .with_single_cert_with_ocsp(certs, privkey, ocsp)
    .context("Bad certificates/private key")?;

    config.key_log = Arc::new(rustls::KeyLogFile::new());

    Ok(Arc::new(config))
}

/// Get subject alternative name from certificate
/// In case of multiple subject alternative names, return None
pub fn get_subj_alt_names(cert: &x509_parser::certificate::X509Certificate) -> Result<String> {
    let extensions = cert.extensions();
    let mut alt_name = None;
    for extension in extensions.iter() {
        debug!("extension: {:?}", extension.parsed_extension());
        let extension = extension.parsed_extension();
        if let ParsedExtension::SubjectAlternativeName(SubjectAlternativeName { general_names }) =
            extension
        {
            for general_name in general_names {
                if let RFC822Name(email) = &general_name {
                    debug!("Email: {:?}", email);
                    if alt_name.is_some() {
                        // If multiple alt name, return err
                        return Err(anyhow!("Unsupported multiple subject alternative name",));
                    }
                    alt_name = Some(email.to_string());
                }
            }
        }
    }
    match alt_name {
        Some(alt_name) => Ok(alt_name),
        None => Err(anyhow!("No subject alternative name found",)),
    }
}

/// Make tls client config from config file
pub fn make_client_config(
    ca_file: Option<&str>,
    client_cert: Option<&str>,
    client_key: Option<&str>,
) -> Result<Arc<rustls::ClientConfig>> {
    let mut root_store = RootCertStore::empty();

    if let Some(ca_file) = ca_file {
        let certfile = fs::File::open(ca_file).context("Cannot open CA file")?;
        let mut reader = BufReader::new(certfile);
        let mut certs = vec![];
        for cert in rustls_pemfile::certs(&mut reader) {
            certs.push(cert.context("Error in parse certificate")?);
        }
        root_store.add_parsable_certificates(certs);
    } else {
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
    }

    let suites = rustls::crypto::ring::DEFAULT_CIPHER_SUITES.to_vec();

    let versions = rustls::DEFAULT_VERSIONS.to_vec();

    let config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::CryptoProvider {
            cipher_suites: suites,
            ..rustls::crypto::ring::default_provider()
        }
        .into(),
    )
    .with_protocol_versions(&versions)
    .context("Inconsistent cipher-suite/versions selected")?
    .with_root_certificates(root_store);

    let mut config = match (client_cert, client_key) {
        (Some(client_cert), Some(client_key)) => {
            let certs = load_certs(client_cert).context("Cannot load ca certificates")?;
            let key = load_private_key(client_key).context("Cannot load private key")?;
            config
                .with_client_auth_cert(certs, key)
                .context("Invalid client auth certs/key")?
        }
        (None, None) => config.with_no_client_auth(),
        _ => {
            panic!("Give both client cert/key");
        }
    };

    config.key_log = Arc::new(rustls::KeyLogFile::new());
    config.enable_sni = false;
    Ok(Arc::new(config))
}
