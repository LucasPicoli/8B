//! The profile write packets of nine captured saves, rebuilt from the saved images.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::devices::ultimate::tables::BLOB_SIZE;
use controller_core::protocol::framing::Framing;
use controller_core::protocol::wire_write::{build_write_packet, PACKET_LEN};
use controller_core::transport::write_input::plan_profile_chunks;

/// The image of save `n`.
fn save(n: usize) -> Vec<u8> {
    std::fs::read(format!("../../fixtures/ultimate/saves/{n:02}.bin")).unwrap()
}

/// Each captured write packet, as the wire carried it.
fn captured() -> Vec<[u8; PACKET_LEN]> {
    let text = std::fs::read_to_string("../../fixtures/ultimate/saves/write-packets.txt").unwrap();
    text.lines()
        .map(|line| {
            let bytes: Vec<u8> = (0..line.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
                .collect();
            bytes.try_into().unwrap()
        })
        .collect()
}

/// The packets that write `image`, framed for the wire.
fn packets(image: &[u8]) -> Vec<[u8; PACKET_LEN]> {
    let total = u16::try_from(BLOB_SIZE).unwrap();
    plan_profile_chunks(BLOB_SIZE)
        .into_iter()
        .map(|(offset, size)| {
            let data = &image[usize::from(offset)..usize::from(offset) + size];
            Framing::Length.request(&build_write_packet(offset, data, total))
        })
        .collect()
}

#[test]
fn every_save_goes_out_as_captured() {
    let wire = captured();
    assert_eq!(wire.len(), 335);
    let mut built = Vec::new();
    for n in 1..=9 {
        let image = packets(&save(n));
        assert_eq!(image.len(), 37);
        let found = wire.windows(image.len()).any(|w| w == image.as_slice());
        assert!(found, "save {n} is not a run of the captured packets");
        built.extend(image);
    }
    // The two packets left over repeat chunks of a save the app sent again.
    assert!(wire.iter().all(|p| built.contains(p)), "a captured packet no save explains");
}
