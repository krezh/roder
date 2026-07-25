use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use rustls::{DistinguishedName as RustlsDistinguishedName, Error as RustlsError};
use sha2::{Digest, Sha256};
use thiserror::Error;

const FINGERPRINT_ANNOTATION: &str = "roder.io/tls-fingerprint";

#[derive(Debug, Error)]
pub enum MtlsError {
    #[error("rcgen: {0}")]
    Rcgen(String),
    #[error("rustls: {0}")]
    Rustls(String),
}

pub struct PeerCert {
    pub der: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    pub fingerprint: String,
}

impl PeerCert {
    pub fn mint(pod_name: &str) -> Result<Self, MtlsError> {
        let mut params = CertificateParams::new(vec![pod_name.to_string()])
            .map_err(|e| MtlsError::Rcgen(e.to_string()))?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, pod_name);
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(3650);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];

        let key = KeyPair::generate_for(&rcgen::PKCS_ED25519)
            .map_err(|e| MtlsError::Rcgen(e.to_string()))?;
        let cert = params
            .self_signed(&key)
            .map_err(|e| MtlsError::Rcgen(e.to_string()))?;
        let der = cert.der().clone();
        let key = PrivateKeyDer::from(key);
        let fingerprint = hex_sha256(der.as_ref());
        Ok(Self {
            der,
            key,
            fingerprint,
        })
    }
}

pub fn annotation_key() -> &'static str {
    FINGERPRINT_ANNOTATION
}

#[derive(Debug)]
pub struct PinnedVerifier {
    fingerprints_by_server: RwLock<HashMap<String, String>>,
}

impl PinnedVerifier {
    pub fn new() -> Self {
        Self {
            fingerprints_by_server: RwLock::new(HashMap::new()),
        }
    }

    pub fn set(&self, fingerprints_by_server: HashMap<String, String>) {
        *self
            .fingerprints_by_server
            .write()
            .expect("peer fingerprint lock poisoned") = fingerprints_by_server;
    }

    fn verify_pinned_server(
        &self,
        certificate: &CertificateDer<'_>,
        server_name: &ServerName<'_>,
    ) -> Result<(), RustlsError> {
        let expected = self
            .fingerprints_by_server
            .read()
            .map_err(|_| RustlsError::General("peer fingerprint lock poisoned".into()))?
            .get(server_name.to_str().as_ref())
            .cloned()
            .ok_or_else(|| RustlsError::General("server is not a trusted Roder peer".into()))?;
        verify_fingerprint(certificate, &expected)
    }

    fn verify_pinned_client(&self, certificate: &CertificateDer<'_>) -> Result<(), RustlsError> {
        let presented = hex_sha256(certificate.as_ref());
        let trusted = self
            .fingerprints_by_server
            .read()
            .map_err(|_| RustlsError::General("peer fingerprint lock poisoned".into()))?
            .values()
            .any(|fingerprint| fingerprint == &presented);
        if trusted {
            Ok(())
        } else {
            Err(RustlsError::General(
                "client is not a trusted Roder peer".into(),
            ))
        }
    }

    fn verify_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        if dss.scheme != SignatureScheme::ED25519 {
            return Err(RustlsError::General(
                "Roder peer certificate did not use Ed25519".into(),
            ));
        }
        let provider = rustls::crypto::ring::default_provider();
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &provider.signature_verification_algorithms,
        )
    }
}

impl Default for PinnedVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.verify_pinned_server(end_entity, server_name)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "TLS 1.2 not supported for peer mTLS".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

impl ClientCertVerifier for PinnedVerifier {
    fn root_hint_subjects(&self) -> &[RustlsDistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, RustlsError> {
        self.verify_pinned_client(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General(
            "TLS 1.2 not supported for peer mTLS".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

pub fn server_config(
    cert: &PeerCert,
    verifier: Arc<PinnedVerifier>,
) -> Result<ServerConfig, MtlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| MtlsError::Rustls(e.to_string()))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert.der.clone()], cert.key.clone_key())
        .map_err(|e| MtlsError::Rustls(e.to_string()))
}

pub fn client_config(
    cert: &PeerCert,
    verifier: Arc<PinnedVerifier>,
) -> Result<ClientConfig, MtlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| MtlsError::Rustls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![cert.der.clone()], cert.key.clone_key())
        .map_err(|e| MtlsError::Rustls(e.to_string()))
}

fn verify_fingerprint(cert: &CertificateDer<'_>, expected: &str) -> Result<(), RustlsError> {
    let presented = hex_sha256(cert.as_ref());
    if presented == expected {
        Ok(())
    } else {
        Err(RustlsError::General(
            "server certificate fingerprint does not match its Roder pod".into(),
        ))
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn minted_certs_have_distinct_fingerprints() {
        let a = PeerCert::mint("pod-a").unwrap();
        let b = PeerCert::mint("pod-b").unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn fingerprint_is_64_lowercase_hex_chars() {
        let cert = PeerCert::mint("pod").unwrap();
        assert_eq!(cert.fingerprint.len(), 64);
        assert!(cert.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn trusted_peers_complete_a_mutual_tls_handshake() {
        let client_cert = PeerCert::mint("pod-a").unwrap();
        let server_cert = PeerCert::mint("pod-b").unwrap();
        let client_verifier = Arc::new(PinnedVerifier::new());
        client_verifier.set(HashMap::from([(
            "10.0.0.2".into(),
            server_cert.fingerprint.clone(),
        )]));
        let server_verifier = Arc::new(PinnedVerifier::new());
        server_verifier.set(HashMap::from([(
            "10.0.0.1".into(),
            client_cert.fingerprint.clone(),
        )]));

        let client = client_config(&client_cert, client_verifier).unwrap();
        let server = server_config(&server_cert, server_verifier).unwrap();
        complete_handshake(client, server, "10.0.0.2").unwrap();
    }

    #[test]
    fn server_certificate_is_bound_to_destination_ip() {
        let client_cert = PeerCert::mint("pod-a").unwrap();
        let server_cert = PeerCert::mint("pod-b").unwrap();
        let client_verifier = Arc::new(PinnedVerifier::new());
        client_verifier.set(HashMap::from([(
            "10.0.0.3".into(),
            server_cert.fingerprint.clone(),
        )]));
        let server_verifier = Arc::new(PinnedVerifier::new());
        server_verifier.set(HashMap::from([(
            "10.0.0.1".into(),
            client_cert.fingerprint.clone(),
        )]));

        let client = client_config(&client_cert, client_verifier).unwrap();
        let server = server_config(&server_cert, server_verifier).unwrap();
        assert!(complete_handshake(client, server, "10.0.0.2").is_err());
    }

    #[test]
    fn server_rejects_an_untrusted_client_certificate() {
        let client_cert = PeerCert::mint("pod-a").unwrap();
        let server_cert = PeerCert::mint("pod-b").unwrap();
        let client_verifier = Arc::new(PinnedVerifier::new());
        client_verifier.set(HashMap::from([(
            "10.0.0.2".into(),
            server_cert.fingerprint.clone(),
        )]));
        let server_verifier = Arc::new(PinnedVerifier::new());

        let client = client_config(&client_cert, client_verifier).unwrap();
        let server = server_config(&server_cert, server_verifier).unwrap();
        assert!(complete_handshake(client, server, "10.0.0.2").is_err());
    }

    fn complete_handshake(
        client_config: ClientConfig,
        server_config: ServerConfig,
        server_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server_name = ServerName::try_from(server_name.to_owned())?;
        let mut client = rustls::ClientConnection::new(Arc::new(client_config), server_name)?;
        let mut server = rustls::ServerConnection::new(Arc::new(server_config))?;

        while client.is_handshaking() || server.is_handshaking() {
            let mut client_bytes = Vec::new();
            client.write_tls(&mut client_bytes)?;
            if !client_bytes.is_empty() {
                server.read_tls(&mut Cursor::new(client_bytes))?;
                server.process_new_packets()?;
            }

            let mut server_bytes = Vec::new();
            server.write_tls(&mut server_bytes)?;
            if !server_bytes.is_empty() {
                client.read_tls(&mut Cursor::new(server_bytes))?;
                client.process_new_packets()?;
            }
        }
        Ok(())
    }
}
