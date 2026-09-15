// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Round-trip estricto con CCI sintéticos (rápido, corre en CI).
//!
//! Replica el flujo de parches sobre CCI canónico (caso IE 1-2-3): base
//! descifrada -> cadena de 2 parches estrictos -> hash final. Requiere el
//! binario `xdelta3` (en CI se instala por apt; si falta, el test avisa y
//! pasa sin comprobar).

use ie_core::{cci, normalize, pipeline, xdelta};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn have_xdelta() -> bool {
    xdelta::find_binary().is_ok()
}

/// NCCH mínimo válido descifrado (None) + cuerpo de patrón.
fn fake_ncch(title: u64, body_len: usize, seed: u8) -> Vec<u8> {
    let mut hdr = [0u8; 0x200];
    hdr[0x100..0x104].copy_from_slice(b"NCCH");
    hdr[0x104..0x108].copy_from_slice(&((0x200 + body_len) as u32 / 0x200).to_le_bytes());
    hdr[0x108..0x110].copy_from_slice(&0x0102030405060708u64.to_le_bytes());
    hdr[0x112..0x114].copy_from_slice(&2u16.to_le_bytes());
    hdr[0x118..0x120].copy_from_slice(&title.to_le_bytes());
    hdr[0x150..0x154].copy_from_slice(b"TEST");
    hdr[0x18E] = 0x00; // flags[6] = SizeType 0 -> media 0x200
    hdr[0x18F] = 0x04; // flags[7] = None (descifrado)
    // RomFS mínimo para que el verify (magia IVFC) tenga qué leer.
    hdr[0x1B0..0x1B4].copy_from_slice(&1u32.to_le_bytes());
    hdr[0x1B4..0x1B8].copy_from_slice(&1u32.to_le_bytes());
    let mut out = hdr.to_vec();
    out.extend((0..body_len).map(|i| (i as u8).wrapping_add(seed)));
    out[0x200..0x204].copy_from_slice(b"IVFC");
    out
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ie-strict-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn mutate(path: &Path, offs: &[u64]) {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
    for (i, off) in offs.iter().enumerate() {
        f.seek(SeekFrom::Start(*off)).unwrap();
        f.write_all(&[0xA5 + i as u8; 64]).unwrap();
    }
}

fn run_xdelta_encode(src: &Path, dst_data: &Path, patch: &Path) {
    let xb = xdelta::find_binary().unwrap();
    let st = std::process::Command::new(&xb)
        .arg("-e")
        .arg("-s")
        .arg(src)
        .arg(dst_data)
        .arg(patch)
        .output()
        .unwrap();
    assert!(st.status.success(), "xdelta3 -e falló");
}

#[test]
fn cadena_estricta_y_hash_final() {
    if !have_xdelta() {
        eprintln!("SKIP: sin binario xdelta3");
        return;
    }
    let dir = tmpdir("ok");
    let cancel = AtomicBool::new(false);

    // Base: CCI sintético descifrado (None) de ~2 MB.
    let c0 = dir.join("c0.cxi");
    let c1 = dir.join("c1.cfa");
    std::fs::write(&c0, fake_ncch(0x000400000010BB00, 1 << 20, 1)).unwrap();
    std::fs::write(&c1, fake_ncch(0x000400000010BB01, 0x200, 2)).unwrap();
    let base = dir.join("base.3ds");
    cci::build_cci(&c0, &c1, 0x000400000010BB00, &base, &cancel, &mut |_, _| {}).unwrap();

    // Cadena v1 (un cambio) + v1.1 (dos cambios más), como Luis.
    let b1 = dir.join("b1.3ds");
    std::fs::copy(&base, &b1).unwrap();
    mutate(&b1, &[0x5000]);
    let p1 = dir.join("p1.xdelta");
    run_xdelta_encode(&base, &b1, &p1);
    let b2 = dir.join("b2.3ds");
    std::fs::copy(&b1, &b2).unwrap();
    mutate(&b2, &[0x9000, 0x12000]);
    let p2 = dir.join("p2.xdelta");
    run_xdelta_encode(&b1, &b2, &p2);

    let want = normalize::sha256_file(&b2, &cancel, &mut |_| {}).unwrap();
    let out = dir.join("out.3ds");
    pipeline::run_strict(
        &pipeline::StrictInputs {
            base: base.clone(),
            patches: vec![p1, p2],
            expected_sha256: Some(want),
            expected_content_sha256: None,
            out_cci: out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&b2).unwrap());

    // Negativo: hash final erróneo debe fallar.
    let out2 = dir.join("out2.3ds");
    let r = pipeline::run_strict(
        &pipeline::StrictInputs {
            base,
            patches: vec![dir.join("p1.xdelta")],
            expected_sha256: Some("00".repeat(32)),
            expected_content_sha256: None,
            out_cci: out2,
            keep_work: false,
        },
        &cancel,
        &mut |_| {},
    );
    assert!(r.is_err(), "hash erróneo debería fallar");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn estricto_rechaza_base_equivocada() {    if !have_xdelta() {
        eprintln!("SKIP: sin binario xdelta3");
        return;
    }
    let dir = tmpdir("neg");
    let cancel = AtomicBool::new(false);
    let c0 = dir.join("c0.cxi");
    let c1 = dir.join("c1.cfa");
    std::fs::write(&c0, fake_ncch(0x000400000010BB00, 1 << 16, 7)).unwrap();
    std::fs::write(&c1, fake_ncch(0x000400000010BB01, 0x200, 8)).unwrap();
    let base = dir.join("base.3ds");
    cci::build_cci(&c0, &c1, 0x000400000010BB00, &base, &cancel, &mut |_, _| {}).unwrap();
    // Parche generado contra OTRA base (contenido distinto).
    let other = dir.join("other.3ds");
    std::fs::copy(&base, &other).unwrap();
    mutate(&other, &[0x4100]);
    let target = dir.join("target.3ds");
    std::fs::copy(&other, &target).unwrap();
    mutate(&target, &[0x4200]);
    let p = dir.join("p.xdelta");
    run_xdelta_encode(&other, &target, &p);
    // Aplicarlo sobre `base` (casi idéntica pero distinta) debe fallar en
    // checksum en vez de producir una quimera silenciosa.
    let mut wrong = std::fs::read(&base).unwrap();
    wrong[0x4300] ^= 0xFF;
    let wrong_base = dir.join("wrong.3ds");
    std::fs::write(&wrong_base, &wrong).unwrap();
    let out = dir.join("out.3ds");
    let r = pipeline::run_strict(
        &pipeline::StrictInputs {
            base: wrong_base,
            patches: vec![p],
            expected_sha256: None,
            expected_content_sha256: None,
            out_cci: out,
            keep_work: false,
        },
        &cancel,
        &mut |_| {},
    );
    match r {
        Err(e) => assert!(e.to_string().contains("xdelta3 rechazó") || e.to_string().contains("checksum") || e.to_string().contains("Parche"), "error inesperado: {e}"),
        Ok(()) => panic!("base equivocada debería fallar en estricto"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// La puerta de contenido ignora el wrapper NCSD (linaje del dump): mismo
/// juego, distinta firma de cabecera -> contenido OK, completo NO.
#[test]
fn puerta_contenido_ignora_wrapper() {
    if !have_xdelta() {
        eprintln!("SKIP: sin binario xdelta3");
        return;
    }
    let dir = tmpdir("wrap");
    let cancel = AtomicBool::new(false);
    let c0 = dir.join("c0.cxi");
    let c1 = dir.join("c1.cfa");
    std::fs::write(&c0, fake_ncch(0x000400000010BB00, 1 << 16, 3)).unwrap();
    std::fs::write(&c1, fake_ncch(0x000400000010BB01, 0x200, 4)).unwrap();
    let base = dir.join("base.3ds");
    cci::build_cci(&c0, &c1, 0x000400000010BB00, &base, &cancel, &mut |_, _| {}).unwrap();
    // Target: un cambio de contenido.
    let target = dir.join("target.3ds");
    std::fs::copy(&base, &target).unwrap();
    mutate(&target, &[0x5000]);
    let p = dir.join("p.xdelta");
    run_xdelta_encode(&base, &target, &p);
    // Variante con solo el wrapper tocado (firma NCSD).
    let mut variant = std::fs::read(&target).unwrap();
    variant[0x10] ^= 0xFF;
    let variant_path = dir.join("variant.3ds");
    std::fs::write(&variant_path, &variant).unwrap();
    let content_gate = normalize::sha256_cci_content(&target, &cancel, &mut |_| {}).unwrap();
    assert_eq!(content_gate, normalize::sha256_cci_content(&variant_path, &cancel, &mut |_| {}).unwrap());
    // Puerta de contenido: pasa aunque el wrapper difiera.
    let out = dir.join("out.3ds");
    pipeline::run_strict(
        &pipeline::StrictInputs {
            base: base.clone(),
            patches: vec![p.clone()],
            expected_sha256: None,
            expected_content_sha256: Some(content_gate),
            out_cci: out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&target).unwrap());
    // Puerta completa contra el hash de la variante: debe fallar.
    let full_variant = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&variant))
    };
    let out2 = dir.join("out2.3ds");
    let r = pipeline::run_strict(
        &pipeline::StrictInputs {
            base,
            patches: vec![p],
            expected_sha256: Some(full_variant),
            expected_content_sha256: None,
            out_cci: out2,
            keep_work: false,
        },
        &cancel,
        &mut |_| {},
    );
    assert!(r.is_err(), "hash completo distinto debería fallar");
    std::fs::remove_dir_all(&dir).ok();
}
