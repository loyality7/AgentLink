use agentlink_core::{
    crypto::{decrypt_from_peer, encrypt_for_peer},
    guardrails::{PathGuard, ProjectConfig},
    identity::Identity,
    protocol::{generate_pairing_code, AgentMessage, MessageType},
};

#[test]
fn identity_and_e2ee_roundtrip() {
    let alice = Identity::generate("alice");
    let bob = Identity::generate("bob");

    let alice_pub = alice.to_public();
    let bob_pub = bob.to_public();
    alice_pub.verify_peer_id().unwrap();
    bob_pub.verify_peer_id().unwrap();

    let msg = AgentMessage::new_question(
        "msg_1",
        "thr_1",
        Some("agentlink".into()),
        "What is the /api/orders response schema?",
    );
    let plaintext = serde_json::to_vec(&msg).unwrap();

    let env = encrypt_for_peer(&alice, &bob_pub, "msg_1", &plaintext).unwrap();
    assert_eq!(env.from, alice.peer_id);
    assert_eq!(env.to, bob.peer_id);

    let decrypted = decrypt_from_peer(&bob, &alice_pub, &env).unwrap();
    let decoded: AgentMessage = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(decoded, msg);
    assert_eq!(decoded.msg_type, MessageType::Question);
    assert!(decoded.can_auto_reply());
}

#[test]
fn tampered_ciphertext_is_rejected() {
    let alice = Identity::generate("alice");
    let bob = Identity::generate("bob");

    let mut env = encrypt_for_peer(&alice, &bob.to_public(), "msg_2", b"secret").unwrap();
    env.ciphertext = "AAAA".into();

    assert!(decrypt_from_peer(&bob, &alice.to_public(), &env).is_err());
}

#[test]
fn pairing_code_and_hop_limits() {
    let code = generate_pairing_code();
    assert!(code.starts_with("AL-"));
    assert_eq!(code.len(), 12);

    let mut msg = AgentMessage::new_question("m1", "t1", None, "ping");
    msg.hop = 5;
    msg.max_hops = 5;
    assert!(!msg.can_auto_reply());
}

#[test]
fn path_guard_blocks_secrets_even_under_allowed_globs() {
    let mut cfg = ProjectConfig::default();
    cfg.allowed_paths = vec!["**/*".into()];
    let guard = PathGuard::from_config(&cfg).unwrap();

    assert!(guard.is_allowed("src/main.rs"));
    assert!(guard.is_allowed("docs/PLAN.md"));
    assert!(!guard.is_allowed(".env"));
    assert!(!guard.is_allowed(".env.local"));
    assert!(!guard.is_allowed("config/secrets/api.json"));
    assert!(!guard.is_allowed("certs/server.pem"));
}

