// Rust side of the differential check for the incremental update_crc_* family.
// Mirrors update_oracle.c enumeration exactly; digests must match bit for bit.
fn mix(acc: &mut u64, v: u64) {
    *acc ^= v
        .wrapping_add(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(*acc << 6)
        .wrapping_add(*acc >> 2);
}

fn main() {
    let mut acc: u64;

    // update_crc_8 : EXHAUSTIVE 256 x 256
    acc = 0;
    for crc in 0u32..256 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_8(crc as u8, byte as u8) as u64,
            );
        }
    }
    println!("update_crc_8 {:016X}", acc);

    // update_crc_16 : EXHAUSTIVE 65536 x 256
    acc = 0;
    for crc in 0u32..65536 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_16(crc as u16, byte as u8) as u64,
            );
        }
    }
    println!("update_crc_16 {:016X}", acc);

    // update_crc_ccitt : EXHAUSTIVE
    acc = 0;
    for crc in 0u32..65536 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_ccitt(crc as u16, byte as u8) as u64,
            );
        }
    }
    println!("update_crc_ccitt {:016X}", acc);

    // update_crc_kermit : EXHAUSTIVE
    acc = 0;
    for crc in 0u32..65536 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_kermit(crc as u16, byte as u8) as u64,
            );
        }
    }
    println!("update_crc_kermit {:016X}", acc);

    // update_crc_dnp : EXHAUSTIVE
    acc = 0;
    for crc in 0u32..65536 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_dnp(crc as u16, byte as u8) as u64,
            );
        }
    }
    println!("update_crc_dnp {:016X}", acc);

    // update_crc_sick : every 257th crc x all bytes x all prev bytes
    acc = 0;
    let mut crc = 0u32;
    while crc < 65536 {
        for byte in 0u32..256 {
            for prev in 0u32..256 {
                mix(
                    &mut acc,
                    libcrc_rs::update_crc_sick(crc as u16, byte as u8, prev as u8) as u64,
                );
            }
        }
        crc += 257;
    }
    println!("update_crc_sick {:016X}", acc);

    // update_crc_32 : stride sweep x all bytes
    acc = 0;
    let mut c: u64 = 0;
    while c < 0x1_0000_0000 {
        for byte in 0u32..256 {
            mix(
                &mut acc,
                libcrc_rs::update_crc_32(c as u32, byte as u8) as u64,
            );
        }
        c += 0x1_0001;
    }
    println!("update_crc_32 {:016X}", acc);

    // update_crc_64 : stride sweep x all bytes
    acc = 0;
    let mut c: u64 = 0;
    while c < 0xFFFF_FFFF_FFFF_FFFFu64 - 0x1000_0000_0000_0000u64 {
        for byte in 0u32..256 {
            mix(&mut acc, libcrc_rs::update_crc_64(c, byte as u8));
        }
        c = c.wrapping_add(0x1_0001_0001_0001);
    }
    println!("update_crc_64 {:016X}", acc);
}
