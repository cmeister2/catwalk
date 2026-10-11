//! Independent single-frame checksum, truncation and allocation-admission checks for the adapter.
use cadmpeg_core::{
    CodecError,
    decode::{DecodeArena, DecodeContext, DecodePolicy, ResourceDimension, ResourceFailure},
};

/// Expand one admitted frame by repeatedly reading small chunks from the native decode context.
fn expand(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let arena = DecodeArena::new();
    let (ctx, root) = DecodeContext::from_root_bytes(bytes, &arena, &DecodePolicy::service())?;
    let mut decoder = ctx.open_zstd(root)?;
    let mut result = Vec::new();
    let mut chunk = [0u8; 16];
    loop {
        let read = decoder.read_chunk(&mut chunk)?;
        if read == 0 {
            break;
        }
        result.extend_from_slice(&chunk[..read]);
    }
    Ok(result)
}

#[test]
fn exact_single_frame_checks_checksum_and_rejects_trailing_data() {
    // Original frame: single segment, three raw bytes, XXH64("abc",0) low32.
    let frame = [
        0x28, 0xb5, 0x2f, 0xfd, 0x24, 3, 0x19, 0, 0, b'a', b'b', b'c', 0x99, 0x09, 0x77, 0xad,
    ];
    assert_eq!(expand(&frame).unwrap(), b"abc");
    let mut damaged = frame;
    damaged[15] ^= 1;
    assert!(
        expand(&damaged)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    assert!(expand(&[frame.as_slice(), b"extra"].concat()).is_err());
    assert!(expand(&[frame.as_slice(), frame.as_slice()].concat()).is_err());
    for end in 0..frame.len() {
        assert!(expand(&frame[..end]).is_err(), "truncated at {end}");
    }
}

#[test]
fn enormous_window_is_refused_before_allocating_the_decoder() {
    let enormous = [0x28, 0xb5, 0x2f, 0xfd, 0, 0xf8];
    let CodecError::ResourceLimit(limit) = expand(&enormous).unwrap_err() else {
        panic!("enormous window must be refused by the admission budget");
    };
    assert_eq!(limit.dimension, ResourceDimension::MaterializedBytes);
    assert_eq!(limit.reason, ResourceFailure::BudgetExceeded);
    assert_eq!(limit.additional, 1_u64 << 41);
}
