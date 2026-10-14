//! Identifiers the runtime makes up for what a service tier would assign.

/// The package id of the app with id `app` (16 bytes in text order).
///
/// A real tier assigns the package ids when it publishes an app, and nothing in a process can know
/// them. The contract the corpus pins is only this: the id is not empty, every object of one app
/// carries the same one (AllObj and AllObjWithCaption agree for one object), and different apps
/// have different ones (record/TestAllObjOwningAppPerKind.al:109-132, TestAllObjVirtualTable.al
/// "OwnTable_PackageIdsEqualAllObj"). A function of the app id gives exactly that, and is the same
/// in every run. The empty app (an object nothing owns) has the empty id.
pub(crate) fn package_id(app: [u8; 16]) -> [u8; 16] {
    if app == [0; 16] {
        return [0; 16];
    }
    // Two 64-bit FNV-1a lanes over the id and a fixed salt, laid out as a version 4 guid.
    let salt = b"al-runtime/package-id/v1";
    let mut out = [0u8; 16];
    for (lane, chunk) in out.chunks_mut(8).enumerate() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (lane as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for b in app.iter().chain(salt) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        chunk.copy_from_slice(&h.to_be_bytes());
    }
    out[6] = (out[6] & 0x0f) | 0x40;
    out[8] = (out[8] & 0x3f) | 0x80;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_id_follows_the_app_and_is_never_empty_for_a_real_app() {
        let (a, b) = ([1u8; 16], [2u8; 16]);
        assert_ne!(package_id(a), [0; 16]);
        assert_eq!(package_id(a), package_id(a));
        assert_ne!(package_id(a), package_id(b));
        assert_ne!(package_id(a), a);
        assert_eq!(package_id([0; 16]), [0; 16]);
    }
}
