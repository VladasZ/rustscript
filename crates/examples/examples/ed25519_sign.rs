#!/usr/bin/env rust

// a fixed key so the output is stable, sign then verify both ways

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hex::encode;

fn main() {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let public = key.verifying_key();
    println!("public {}", encode(public.to_bytes()));

    let message = b"artifact bytes";
    let sig = key.sign(message);
    println!("sig {}", encode(sig.to_bytes()));

    println!("verify ok {}", public.verify(message, &sig).is_ok());
    println!("strict ok {}", public.verify_strict(message, &sig).is_ok());
    println!("tampered ok {}", public.verify(b"tampered", &sig).is_ok());

    let restored = VerifyingKey::from_bytes(&public.to_bytes()).unwrap();
    let restored_sig = Signature::from_bytes(&sig.to_bytes());
    println!(
        "restored ok {}",
        restored.verify(message, &restored_sig).is_ok()
    );

    let from_slice = SigningKey::try_from(key.to_bytes().as_slice()).unwrap();
    println!(
        "same key {}",
        from_slice.verifying_key().to_bytes() == public.to_bytes()
    );
    println!(
        "short key rejected {}",
        SigningKey::try_from([1u8; 5].as_slice()).is_err()
    );
}
