// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Writer `.cia` con datos sintéticos (rápido, corre en CI sin dumps).
//!
//! Monta un CCI de 2 particiones y un CIA donante mínimo del mismo TitleId,
//! llama a `cia::build_from_cci` y comprueba lo que de verdad miran los
//! instaladores: bitmap de contents, ticket falso estilo GM9 y TMD con los
//! records y el contentinfo rehasheados.

use ie_core::cci::{self, CciPart};
use ie_core::cia;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const TITLE: u64 = 0x0004_0000_0017_3C00;

fn align64(n: u64) -> u64 {
    n.next_multiple_of(64)
}

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ie-cia-synth-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// NCCH mínimo descifrado (None) con TitleId en la cabecera.
fn fake_ncch(title: u64, body_len: usize, seed: u8) -> Vec<u8> {
    let mut hdr = [0u8; 0x200];
    hdr[0x100..0x104].copy_from_slice(b"NCCH");
    hdr[0x104..0x108].copy_from_slice(&((0x200 + body_len) as u32 / 0x200).to_le_bytes());
    hdr[0x108..0x110].copy_from_slice(&title.to_le_bytes());
    hdr[0x112..0x114].copy_from_slice(&2u16.to_le_bytes());
    hdr[0x150..0x154].copy_from_slice(b"TEST");
    hdr[0x18E] = 0x00;
    hdr[0x18F] = 0x04; // NCCH descifrado
    let mut out = hdr.to_vec();
    out.extend((0..body_len).map(|i| (i as u8).wrapping_add(seed)));
    out
}

/// CIA donante mínimo: cabecera + cert + ticket + TMD bien formado y los
/// contents contiguos al final (sin meta ni footer).
fn build_donor(path: &Path, title: u64, contents: &[Vec<u8>]) {
    const CERT_LEN: usize = 0xA00;
    const TIK_LEN: usize = 0x350;
    const HEAD: usize = 0x140; // (4 + 0x100 + 63) & !63, firma 0x10004
    let count = contents.len();
    let cib = HEAD + 0xC4;
    let chunks = cib + 64 * 0x24;
    let tmd_len = chunks + count * 0x30;

    let mut tmd = vec![0u8; tmd_len];
    tmd[0..4].copy_from_slice(&0x10004u32.to_be_bytes());
    tmd[HEAD..HEAD + 26].copy_from_slice(b"Root-CA00000003-CP0000000b");
    tmd[HEAD + 0x40] = 1; // version
    tmd[HEAD + 0x4C..HEAD + 0x54].copy_from_slice(&title.to_be_bytes());
    tmd[HEAD + 0x9E..HEAD + 0xA0].copy_from_slice(&(count as u16).to_be_bytes());
    for (i, c) in contents.iter().enumerate() {
        let o = chunks + i * 0x30;
        tmd[o..o + 4].copy_from_slice(&(i as u32).to_be_bytes());
        tmd[o + 4..o + 6].copy_from_slice(&(i as u16).to_be_bytes());
        tmd[o + 6..o + 8].copy_from_slice(&1u16.to_be_bytes());
        tmd[o + 8..o + 16].copy_from_slice(&(c.len() as u64).to_be_bytes());
        tmd[o + 16..o + 48].copy_from_slice(&sha(c));
    }
    // contentinfo: una entrada que cubre los `count` chunks del bloque.
    tmd[cib + 2..cib + 4].copy_from_slice(&(count as u16).to_be_bytes());
    let h = sha(&tmd[chunks..chunks + count * 0x30]);
    tmd[cib + 4..cib + 36].copy_from_slice(&h);
    let master = sha(&tmd[cib..cib + 64 * 0x24]);
    tmd[cib - 0x20..cib].copy_from_slice(&master);

    let cert_off = 0x40u64;
    let tik_off = align64(cert_off + CERT_LEN as u64);
    let tmd_off = align64(tik_off + TIK_LEN as u64);
    let c0 = align64(tmd_off + tmd_len as u64);
    let total: u64 = c0 + contents.iter().map(|c| c.len() as u64).sum::<u64>();

    let mut img = vec![0u8; total as usize];
    img[0..4].copy_from_slice(&0x2020u32.to_le_bytes());
    img[0x08..0x0C].copy_from_slice(&(CERT_LEN as u32).to_le_bytes());
    img[0x0C..0x10].copy_from_slice(&(TIK_LEN as u32).to_le_bytes());
    img[0x10..0x14].copy_from_slice(&(tmd_len as u32).to_le_bytes());
    img[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // sin meta/footer
    img[0x18..0x1C].copy_from_slice(
        &(contents.iter().map(|c| c.len() as u64).sum::<u64>() as u32).to_le_bytes(),
    );
    img[tmd_off as usize..tmd_off as usize + tmd_len].copy_from_slice(&tmd);
    let mut off = c0 as usize;
    for c in contents {
        img[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    std::fs::write(path, &img).unwrap();
}

/// CCI de 2 particiones a partir de un fichero con ambas contiguas.
fn build_cci(base: &Path, out: &Path, len0: u64, len1: u64) {
    let parts = [
        CciPart {
            path: base,
            offset: 0,
            len: len0,
        },
        CciPart {
            path: base,
            offset: len0,
            len: len1,
        },
    ];
    cci::build_cci_packed(&parts, TITLE, out, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
}

#[test]
fn cia_con_ticket_falso_y_tmd_rehasheado() {
    let dir = tmpdir("ok");
    let c0 = fake_ncch(TITLE, 0x1000, 0x11);
    let c1 = fake_ncch(TITLE, 0x2000, 0x22);

    let base = dir.join("base.bin");
    let mut raw = c0.clone();
    raw.extend_from_slice(&c1);
    std::fs::write(&base, &raw).unwrap();
    let cci_path = dir.join("base.3ds");
    build_cci(&base, &cci_path, c0.len() as u64, c1.len() as u64);

    let donor = dir.join("donor.cia");
    build_donor(&donor, TITLE, &[c0.clone(), c1.clone()]);

    let out = dir.join("out.cia");
    cia::build_from_cci(&cci_path, &donor, &out, &AtomicBool::new(false), |_, _| {}).unwrap();

    // Re-parsea como lo haría la app y como lo haría un instalador.
    let layout = cia::read_layout(&out).unwrap();
    assert_eq!(layout.contents.len(), 2);
    assert_eq!(layout.contents[0].size, c0.len() as u64);
    assert_eq!(layout.contents[1].size, c1.len() as u64);

    let img = std::fs::read(&out).unwrap();
    let cert_len = u32::from_le_bytes(img[0x08..0x0C].try_into().unwrap()) as u64;
    let tik_len = u32::from_le_bytes(img[0x0C..0x10].try_into().unwrap()) as u64;
    let tmd_len = u32::from_le_bytes(img[0x10..0x14].try_into().unwrap()) as usize;

    // Bitmap de contents: sin estos bits los instaladores ven 0/N contents.
    let bits = &img[0x20..0x2020];
    assert_eq!(bits[0], 0b1100_0000, "bitmap de contents");
    assert!(bits[1..].iter().all(|b| *b == 0));

    // Ticket falso estilo GM9: firma, ECDSA y titlekey a 0xFF + TitleId real.
    let tik = align64(0x2040 + cert_len) as usize;
    assert!(tik + 0x350 <= img.len());
    assert_eq!(
        &img[tik..tik + 4],
        &0x10004u32.to_be_bytes(),
        "tipo de ticket"
    );
    assert!(
        img[tik + 4..tik + 0x104].iter().all(|b| *b == 0xFF),
        "firma a 0xFF"
    );
    assert!(
        img[tik + 0x180..tik + 0x1BC].iter().all(|b| *b == 0xFF),
        "ecdsa a 0xFF"
    );
    assert!(
        img[tik + 0x1BF..tik + 0x1CF].iter().all(|b| *b == 0xFF),
        "titlekey a 0xFF"
    );
    assert_eq!(
        u64::from_be_bytes(img[tik + 0x1DC..tik + 0x1E4].try_into().unwrap()),
        TITLE
    );

    // TMD: records al día y contentinfo rehasheado (lo que verifica el installer).
    let tmd_off = align64(tik as u64 + tik_len) as usize;
    let tmd = &img[tmd_off..tmd_off + tmd_len];
    assert!(
        tmd[4..0x104].iter().all(|b| *b == 0xFF),
        "firma del TMD a 0xFF"
    );
    let chunks = 0x140 + 0xC4 + 64 * 0x24;
    for (i, src) in [&c0, &c1].iter().enumerate() {
        let o = chunks + i * 0x30;
        let size = u64::from_be_bytes(tmd[o + 8..o + 16].try_into().unwrap());
        assert_eq!(size, src.len() as u64, "tamaño del record {i}");
        assert_eq!(&tmd[o + 16..o + 48], &sha(src), "sha del record {i}");
    }
    let cib = 0x140 + 0xC4;
    let esperado = sha(&tmd[chunks..chunks + 2 * 0x30]);
    assert_eq!(
        &tmd[cib + 4..cib + 36],
        &esperado,
        "hash del bloque de chunks"
    );
    let master = sha(&tmd[cib..cib + 64 * 0x24]);
    assert_eq!(
        &tmd[cib - 0x20..cib],
        &master,
        "master hash del contentinfo"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn donante_de_otro_juego_se_rechaza() {
    let dir = tmpdir("otro");
    let c0 = fake_ncch(TITLE, 0x1000, 0x11);
    let c1 = fake_ncch(TITLE, 0x2000, 0x22);
    let base = dir.join("base.bin");
    let mut raw = c0.clone();
    raw.extend_from_slice(&c1);
    std::fs::write(&base, &raw).unwrap();
    let cci_path = dir.join("base.3ds");
    build_cci(&base, &cci_path, c0.len() as u64, c1.len() as u64);

    let donor = dir.join("otro.cia");
    build_donor(&donor, TITLE + 0x10, &[c0, c1]);
    let out = dir.join("out.cia");
    let err = cia::build_from_cci(&cci_path, &donor, &out, &AtomicBool::new(false), |_, _| {})
        .unwrap_err()
        .to_string();
    assert!(err.contains("otro juego"), "error inesperado: {err}");
    assert!(!out.exists(), "no debe dejar salida a medias");

    std::fs::remove_dir_all(&dir).ok();
}
