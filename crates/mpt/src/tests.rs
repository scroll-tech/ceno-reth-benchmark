use revm_primitives::{b256, keccak256};

use crate::{Error, FrontierLimits, Mpt};

trait RlpBytes {
    /// Returns the RLP-encoding.
    fn to_rlp(&self) -> Vec<u8>;
}

impl<T> RlpBytes for T
where
    T: alloy_rlp::Encodable,
{
    #[inline]
    fn to_rlp(&self) -> Vec<u8> {
        let rlp_length = self.length();
        let mut out = Vec::with_capacity(rlp_length);
        self.encode(&mut out);
        debug_assert_eq!(out.len(), rlp_length);
        out
    }
}

#[test]
fn test_empty() {
    let bump = bumpalo::Bump::new();
    let trie = Mpt::new(&bump);

    assert!(trie.is_empty());
    let expected = b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421");
    assert_eq!(expected, trie.hash());
}

#[test]
fn test_empty_key() -> Result<(), Error> {
    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    trie.insert(&[], b"empty")?;
    assert_eq!(trie.get(&[])?, Some(b"empty".as_ref()));
    assert!(trie.delete(&[])?);

    Ok(())
}

#[test]
fn test_branch_value() {
    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);
    trie.insert(b"do", b"verb").unwrap();
    // leads to a branch with value which is not supported
    trie.insert(b"dog", b"puppy").unwrap_err();
}

#[test]
fn test_insert() -> Result<(), Error> {
    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    let key_vals = vec![
        ("painting", "place"),
        ("guest", "ship"),
        ("mud", "leave"),
        ("paper", "call"),
        ("gate", "boast"),
        ("tongue", "gain"),
        ("baseball", "wait"),
        ("tale", "lie"),
        ("mood", "cope"),
        ("menu", "fear"),
    ];
    for (key, val) in &key_vals {
        assert!(trie.insert(key.as_bytes(), val.as_bytes())?);
    }

    let expected = b256!("2bab6cdf91a23ebf3af683728ea02403a98346f99ed668eec572d55c70a4b08f");
    assert_eq!(expected, trie.hash());

    for (key, value) in &key_vals {
        let retrieved = trie.get(key.as_bytes())?.unwrap();
        assert_eq!(retrieved, value.as_bytes());
    }

    // check inserting duplicate keys
    assert!(trie.insert(key_vals[0].0.as_bytes(), b"new")?);
    assert!(!trie.insert(key_vals[0].0.as_bytes(), b"new")?);

    Ok(())
}

#[test]
fn test_keccak_trie() -> Result<(), Error> {
    const N: usize = 512;

    // insert
    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    for i in 0..N {
        assert!(trie.insert_rlp(keccak256(i.to_be_bytes()).as_slice(), i)?);

        // check hash against trie build in reverse
        let bump2 = bumpalo::Bump::new();
        let mut trie2 = Mpt::new(&bump2);
        for j in (0..=i).rev() {
            trie2.insert_rlp(keccak256(j.to_be_bytes()).as_slice(), j)?;
        }
        assert_eq!(trie.hash(), trie2.hash());
    }

    let expected = b256!("7310027edebdd1f7c950a7fb3413d551e85dff150d45aca4198c2f6315f9b4a7");
    assert_eq!(trie.hash(), expected);

    // get
    for i in 0..N {
        assert_eq!(trie.get_rlp(keccak256(i.to_be_bytes()).as_slice())?, Some(i));
        assert!(trie.get(keccak256((i + N).to_be_bytes()).as_slice())?.is_none());
    }

    // delete
    for i in 0..N {
        assert!(trie.delete(keccak256(i.to_be_bytes()).as_slice())?);

        let bump2 = bumpalo::Bump::new();
        let mut trie2 = Mpt::new(&bump2);
        for j in ((i + 1)..N).rev() {
            trie2.insert_rlp(keccak256(j.to_be_bytes()).as_slice(), j)?;
        }
        assert_eq!(trie.hash(), trie2.hash());
    }
    assert!(trie.is_empty());

    Ok(())
}

#[test]
fn test_index_trie() -> Result<(), Error> {
    const N: usize = 512;

    // insert
    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    for i in 0..N {
        assert!(trie.insert_rlp(&i.to_rlp(), i)?);

        // check hash against trie build in reverse
        let bump2 = bumpalo::Bump::new();
        let mut trie2 = Mpt::new(&bump2);
        for j in (0..=i).rev() {
            trie2.insert_rlp(&j.to_rlp(), j)?;
        }
        assert_eq!(trie.hash(), trie2.hash());
    }

    // get
    for i in 0..N {
        assert_eq!(trie.get_rlp(&i.to_rlp())?, Some(i));
        assert!(trie.get(&(i + N).to_rlp())?.is_none());
    }

    // delete
    for i in 0..N {
        assert!(trie.delete(&i.to_rlp()).unwrap());

        let bump2 = bumpalo::Bump::new();
        let mut trie2 = Mpt::new(&bump2);
        for j in ((i + 1)..N).rev() {
            trie2.insert_rlp(&j.to_rlp(), j)?;
        }
        assert_eq!(trie.hash(), trie2.hash());
    }
    assert!(trie.is_empty());

    Ok(())
}

#[cfg(feature = "host")]
#[test]
fn test_serde_index_trie() -> Result<(), Error> {
    const N: usize = 512;

    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    for i in 0..N {
        assert!(trie.insert_rlp(&i.to_rlp(), i)?);
    }

    let root_hash = trie.hash();

    let encoded = trie.encode_trie();

    let recovered_trie = Mpt::decode_trie(&bump, &mut encoded.as_slice(), trie.num_nodes())?;
    assert_eq!(recovered_trie.hash(), root_hash);

    for i in 0..N {
        let value = recovered_trie.get_rlp(&i.to_rlp())?;
        assert_eq!(value, Some(i));
    }

    Ok(())
}

#[cfg(feature = "host")]
#[test]
fn test_serde_keccak_trie() -> Result<(), Error> {
    const N: usize = 512;

    let bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&bump);

    for i in 0..N {
        assert!(trie.insert_rlp(keccak256(i.to_be_bytes()).as_slice(), i)?);
    }

    let root_hash = trie.hash();

    let encoded = trie.encode_trie();

    let recovered_trie = Mpt::decode_trie(&bump, &mut encoded.as_slice(), trie.num_nodes())?;
    assert_eq!(recovered_trie.hash(), root_hash);

    for i in 0..N {
        let value = recovered_trie.get_rlp(keccak256(i.to_be_bytes()).as_slice())?;
        assert_eq!(value, Some(i));
    }

    Ok(())
}

#[cfg(feature = "host")]
#[test]
fn test_frontier_chunks_match_full_trie_updates() -> Result<(), Error> {
    let bump = bumpalo::Bump::new();
    let mut original = Mpt::new(&bump);
    let keys = (0u64..64).map(|i| keccak256(i.to_be_bytes())).collect::<Vec<_>>();
    for (i, key) in keys.iter().enumerate() {
        original.insert_rlp(key.as_slice(), [i as u8; 64])?;
    }

    let changed = vec![keys[3], keys[17], keys[41]];
    let bundle = original.build_frontier(
        changed.clone(),
        FrontierLimits { max_nodes: 8, max_bytes: 1024, max_keys: 1 },
    )?;
    assert!(bundle.chunks.len() >= 2);

    let frontier_bump = bumpalo::Bump::new();
    let mut frontier_bytes = bundle.bytes.as_slice();
    let mut frontier = Mpt::decode_trie(&frontier_bump, &mut frontier_bytes, bundle.num_nodes)?;
    assert_eq!(frontier.hash(), original.hash());

    for chunk in &bundle.chunks {
        let chunk_bump = bumpalo::Bump::new();
        let mut bytes = chunk.bytes.as_slice();
        let mut trie = Mpt::decode_trie(&chunk_bump, &mut bytes, chunk.num_nodes)?;
        assert_eq!(trie.hash(), chunk.expected_root);
        for key in changed.iter().filter(|key| {
            let nibs = crate::hp::to_nibs(key.as_slice());
            nibs.starts_with(&chunk.prefix)
        }) {
            let nibs = crate::hp::to_nibs(key.as_slice());
            if *key == changed[0] {
                trie.delete_nibbles(&nibs[chunk.prefix.len()..])?;
            } else {
                trie.insert_rlp_nibbles(&nibs[chunk.prefix.len()..], [0xcdu8; 64])?;
            }
        }
        assert!(trie.root_is_hash_addressed());
        frontier.replace_frontier_slot(&chunk.prefix, chunk.expected_root, trie.hash())?;
    }

    original.delete(changed[0].as_slice())?;
    for key in changed.into_iter().skip(1) {
        original.insert_rlp(key.as_slice(), [0xcdu8; 64])?;
    }
    assert_eq!(frontier.hash(), original.hash());
    Ok(())
}

#[cfg(feature = "host")]
#[test]
fn test_frontier_supports_empty_slots_and_rejects_wrong_digest() -> Result<(), Error> {
    let bump = bumpalo::Bump::new();
    let mut original = Mpt::new(&bump);
    let existing =
        revm_primitives::b256!("1000000000000000000000000000000000000000000000000000000000000000");
    let existing_two =
        revm_primitives::b256!("2000000000000000000000000000000000000000000000000000000000000000");
    let inserted =
        revm_primitives::b256!("f000000000000000000000000000000000000000000000000000000000000000");
    original.insert_rlp(existing.as_slice(), [7u8; 64])?;
    original.insert_rlp(existing_two.as_slice(), [8u8; 64])?;

    let bundle = original
        .build_frontier([inserted], FrontierLimits { max_nodes: 0, max_bytes: 0, max_keys: 0 })?;
    assert_eq!(bundle.chunks.len(), 1);
    let chunk = &bundle.chunks[0];
    assert_eq!(chunk.expected_root, alloy_trie::EMPTY_ROOT_HASH);

    let frontier_bump = bumpalo::Bump::new();
    let mut frontier_bytes = bundle.bytes.as_slice();
    let mut frontier = Mpt::decode_trie(&frontier_bump, &mut frontier_bytes, bundle.num_nodes)?;
    assert!(matches!(
        frontier.replace_frontier_slot(
            &chunk.prefix,
            revm_primitives::B256::ZERO,
            revm_primitives::B256::ZERO
        ),
        Err(Error::NodeRefMismatch)
    ));

    let chunk_bump = bumpalo::Bump::new();
    let mut trie = Mpt::new(&chunk_bump);
    let nibs = crate::hp::to_nibs(inserted.as_slice());
    trie.insert_rlp_nibbles(&nibs[chunk.prefix.len()..], [9u8; 64])?;
    frontier.replace_frontier_slot(&chunk.prefix, chunk.expected_root, trie.hash())?;
    original.insert_rlp(inserted.as_slice(), [9u8; 64])?;
    assert_eq!(frontier.hash(), original.hash());
    Ok(())
}
