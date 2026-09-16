// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Modo manifiesto sobre un CCI sintético mínimo (rápido, apto para CI con
//! xdelta3 en el PATH): 1 partición, RomFS con 2 ficheros, pack de 1
//! reemplazo con cambio de tamaño. Verifica ensamblaje, shifts y contenido.

use ie_core::{cci, manifest, pipeline};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

/// Construye un CCI mínimo con RomFS IVFC REAL (1 partición, 2 ficheros,
/// bloques de 0x1000): superblock + master + datos(lvl3: cabecera + FileMeta
/// + FileData) + lvl1 + lvl2, todo hasheado bottom-up como haría makerom.
/// Pack de 1 reemplazo con cambio de tamaño (f1 crece; f2 se reempaqueta).
fn build_synth(dir: &Path) -> (PathBuf, u64, u64) {
    const BL: u32 = 12; // bloques de 0x1000 como en los juegos reales
    // --- nivel de datos: cabecera + filemeta(2 entradas) + filedata ---
    // entrada: parent, sib, doff, len, samehash, nlen, nombre (+pad a 4)
    fn entry(name: &str, doff: u64, len: u64) -> Vec<u8> {
        let w: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let mut e = vec![0u8; 0x20];
        e[8..16].copy_from_slice(&doff.to_le_bytes());
        e[16..24].copy_from_slice(&len.to_le_bytes());
        e[28..32].copy_from_slice(&(w.len() as u32).to_le_bytes());
        e.extend_from_slice(&w);
        while e.len() % 4 != 0 {
            e.push(0);
        }
        e
    }
    // f1 @0 len 5 ("AAAAA"), f2 @8 len 4 ("1234", con hueco como en roms reales)
    let mut filemeta = entry("f1", 0, 5);
    filemeta.extend_from_slice(&entry("f2", 8, 4));
    let fdo = (0x28 + filemeta.len()) as u64;
    let mut lvl3 = vec![0u8; 0x28];
    lvl3[0..4].copy_from_slice(&0x28u32.to_le_bytes());
    lvl3[4..8].copy_from_slice(&0x28u32.to_le_bytes()); // dirhash (vacía)
    lvl3[12..16].copy_from_slice(&0x28u32.to_le_bytes()); // dirmeta (vacía)
    lvl3[20..24].copy_from_slice(&0x28u32.to_le_bytes()); // filehash (vacía)
    lvl3[28..32].copy_from_slice(&0x28u32.to_le_bytes()); // filemeta aquí
    lvl3[32..36].copy_from_slice(&(filemeta.len() as u32).to_le_bytes());
    lvl3[36..40].copy_from_slice(&(fdo as u32).to_le_bytes());
    lvl3.extend_from_slice(&filemeta);
    let file_data_base_rel = lvl3.len() as u64; // == fdo
    assert_eq!(file_data_base_rel, fdo);
    lvl3.extend_from_slice(b"AAAAAZZZ1234"); // f1 @0, hueco, f2 @8
    let s3 = lvl3.len() as u64;
    // --- niveles de hash bottom-up ---
    let e3 = s3.next_multiple_of(1u64 << BL) >> BL;
    let lvl3p = {
        let mut v = lvl3.clone();
        v.resize((e3 << BL) as usize, 0);
        v
    };
    let mut lvl2 = Vec::new();
    for i in 0..e3 {
        lvl2.extend_from_slice(&Sha256::digest(&lvl3p[(i << BL) as usize..((i + 1) << BL) as usize]));
    }
    let s2 = lvl2.len() as u64;
    let b2 = s2.next_multiple_of(1u64 << BL) >> BL;
    let mut lvl2p = lvl2.clone();
    lvl2p.resize((b2 << BL) as usize, 0);
    let mut lvl1 = Vec::new();
    for i in 0..b2 {
        lvl1.extend_from_slice(&Sha256::digest(&lvl2p[(i << BL) as usize..((i + 1) << BL) as usize]));
    }
    let s1 = lvl1.len() as u64;
    let b1 = s1.next_multiple_of(1u64 << BL) >> BL;
    let mut lvl1p = lvl1.clone();
    lvl1p.resize((b1 << BL) as usize, 0);
    let mut master = Vec::new();
    for i in 0..b1 {
        master.extend_from_slice(&Sha256::digest(&lvl1p[(i << BL) as usize..((i + 1) << BL) as usize]));
    }
    let sm = master.len() as u64;
    // --- superblock + ensamblaje del blob ---
    let al = |v: u64| v.next_multiple_of(1u64 << BL);
    let o3 = al(0x60 + sm);
    let o1 = o3 + al(s3);
    let o2 = o1 + al(s1);
    let total = (o2 + al(s2)).next_multiple_of(0x200);
    let mut romfs = vec![0u8; total as usize];
    romfs[0..8].copy_from_slice(b"IVFC\x00\x00\x01\x00");
    romfs[8..12].copy_from_slice(&(sm as u32).to_le_bytes());
    romfs[20..28].copy_from_slice(&s1.to_le_bytes());
    romfs[28..32].copy_from_slice(&BL.to_le_bytes());
    romfs[44..52].copy_from_slice(&s2.to_le_bytes());
    romfs[52..56].copy_from_slice(&BL.to_le_bytes());
    romfs[68..76].copy_from_slice(&s3.to_le_bytes());
    romfs[76..80].copy_from_slice(&BL.to_le_bytes());
    romfs[0x60..0x60 + master.len()].copy_from_slice(&master);
    romfs[o3 as usize..o3 as usize + lvl3.len()].copy_from_slice(&lvl3);
    romfs[o1 as usize..o1 as usize + lvl1.len()].copy_from_slice(&lvl1);
    romfs[o2 as usize..o2 as usize + lvl2.len()].copy_from_slice(&lvl2);
    let romfs_len = romfs.len() as u64;
    let _file_data_base = o3 + fdo;

    // --- NCCH ---
    let exefs = vec![0u8; 0x200];
    let ncch_off = 0x4000u64;
    let exefs_off = 0x200u64; // relativo a partición
    let romfs_off = exefs_off + 0x200;
    let mut ncch = vec![0u8; 0x200];
    ncch[0x100..0x104].copy_from_slice(b"NCCH");
    ncch[0x108..0x110].copy_from_slice(&0x00040000000EDF00u64.to_le_bytes());
    ncch[0x150..0x15A].copy_from_slice(b"CTR-P-AETJ");
    ncch[0x18E] = 0; // media 0x200
    ncch[0x18F] = 0x04; // NoEncrypto -> descifrado
    ncch[0x1A0..0x1A4].copy_from_slice(&1u32.to_le_bytes()); // exefs 1 media
    ncch[0x1A4..0x1A8].copy_from_slice(&1u32.to_le_bytes());
    ncch[0x1A8..0x1AC].copy_from_slice(&1u32.to_le_bytes()); // hash exefs: 0x200
    ncch[0x1B0..0x1B4].copy_from_slice(&((romfs_off / 0x200) as u32).to_le_bytes());
    ncch[0x1B4..0x1B8].copy_from_slice(&((romfs_len / 0x200) as u32).to_le_bytes());
    ncch[0x1B8..0x1BC].copy_from_slice(&1u32.to_le_bytes()); // hash romfs: 0x200 (sintético)
    assert_eq!(romfs_len % 0x200, 0, "el blob sintético debe ir alineado");
    // hashes de superblock reales (como los calcularía makerom)
    {
        use sha2::{Digest, Sha256};
        ncch[0x1C0..0x1E0].copy_from_slice(&Sha256::digest(&exefs[..0x200]));
        ncch[0x1E0..0x200].copy_from_slice(&Sha256::digest(&romfs[..0x200]));
    }

    let p0_len = romfs_off + romfs_len;
    assert_eq!(p0_len % 0x200, 0);
    // --- CCI ---
    let cci = dir.join("base.3ds");
    {
        let mut f = std::fs::File::create(&cci).unwrap();
        f.write_all(&vec![0u8; 0x4000]).unwrap();
        f.seek(SeekFrom::Start(ncch_off)).unwrap();
        f.write_all(&ncch).unwrap();
        f.write_all(&exefs).unwrap();
        f.write_all(&romfs).unwrap();
        f.flush().unwrap();
    }
    // cabecera NCSD real (1 slot)
    {
        let mut hdr = [0u8; 0x200];
        cci::write_ncsd_header(
            &mut hdr,
            &[(ncch_off / 0x200, p0_len / 0x200)],
            0x00040000000EDF00,
            cci::card_media_size(0x4000 + p0_len).unwrap(),
        );
        let mut f = std::fs::OpenOptions::new().write(true).open(&cci).unwrap();
        f.write_all(&hdr).unwrap();
    }
    (cci, romfs_off, romfs_len)
}

/// Genera un .xdelta con el binario del sistema (como haría el traductor).
fn make_patch(src: &[u8], dst: &[u8], dir: &Path, name: &str) -> PathBuf {
    let s = dir.join(format!("{name}.src"));
    let d = dir.join(format!("{name}.dst"));
    let p = dir.join(format!("{name}.xdelta"));
    std::fs::write(&s, src).unwrap();
    std::fs::write(&d, dst).unwrap();
    let st = std::process::Command::new("xdelta3")
        .args(["-e", "-s"])
        .arg(&s)
        .arg(&d)
        .arg(&p)
        .status()
        .expect("xdelta3 en PATH para este test");
    assert!(st.success());
    p
}

#[test]
fn pack_sintetico_end_to_end() {
    let dir = std::env::temp_dir().join(format!("ie-msyn-{}.d", std::process::id()));
    eprintln!("DIR={}", dir.display());
    std::fs::create_dir_all(&dir).unwrap();
    let (base, _, _) = build_synth(&dir);

    // Pack: f1 AAAAA -> 7 bytes (crece; f2 debe desplazarse).
    let new_f1 = b"HHHHHHH";
    let patch = make_patch(b"AAAAA", new_f1, &dir, "f1");
    let pack = dir.join("pack");
    std::fs::create_dir_all(pack.join("parches")).unwrap();
    std::fs::copy(&patch, pack.join("parches/f1.xdelta")).unwrap();
    let mf = format!(
        r#"{{"version":"t","base":"s","ficheros":[{{"ruta":"f1","original_size":5,"original_sha256":"{o}","resultado_size":7,"resultado_sha256":"{r}","parche":"parches/f1.xdelta"}}]}}"#,
        o = sha(b"AAAAA"),
        r = sha(new_f1)
    );
    std::fs::write(pack.join("manifiesto.json"), mf).unwrap();
    // el manifiesto también se valida por la GUI
    assert!(manifest::describe(&pack).is_ok());

    let out = dir.join("out.3ds");
    let cancel = AtomicBool::new(false);
    pipeline::run_manifest(
        &pipeline::ManifestInputs {
            base: base.clone(),
            pack_dir: pack.clone(),
            out_cci: out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut |p| {
            eprintln!("etapa {}/{}: {} {}/{}", p.stage_idx + 1, p.stage_count, p.stage, p.done, p.total);
        },
    )
    .expect("run_manifest sintetico");

    // Verifica: particiones, IVFC no aplica aquí (blob propio), contenido.
    let slots = cci::partitions(&out).unwrap();
    assert_eq!(slots.len(), 1);
    let (p0, _) = slots[0];
    let mut f = std::fs::File::open(&out).unwrap();
    // NCCH intacto
    f.seek(SeekFrom::Start(p0 + 0x100)).unwrap();
    let mut mg = [0u8; 4];
    f.read_exact(&mut mg).unwrap();
    assert_eq!(&mg, b"NCCH");
    // Lee el RomFS nuevo: re-deriva la base de datos desde el superblock,
    // comprueba f1 nuevo + f2 reempaquetado, y re-verifica la cadena IVFC
    // entera (lo que mira GodMode9 a fondo).
    let mut nh = [0u8; 0x200];
    f.seek(SeekFrom::Start(p0)).unwrap();
    f.read_exact(&mut nh).unwrap();
    let media = 1u64 << (nh[0x18E] + 9);
    let rmo = u32::from_le_bytes(nh[0x1B0..0x1B4].try_into().unwrap()) as u64 * media;
    let rms = u32::from_le_bytes(nh[0x1B4..0x1B8].try_into().unwrap()) as u64 * media;
    // Content size == partición 0 (lo que mira GodMode9/consola).
    let csize = u32::from_le_bytes(nh[0x104..0x108].try_into().unwrap()) as u64 * media;
    let (pp0, pp0len) = cci::partitions(&out).unwrap()[0];
    assert_eq!(p0, pp0);
    assert_eq!(csize, pp0len, "NCCH content size debe igualar la partición");
    // Base hash del RomFS coherente con el superblock nuevo.
    let roh = u32::from_le_bytes(nh[0x1B8..0x1BC].try_into().unwrap()) as u64;
    let mut base_reg = vec![0u8; (roh * media) as usize];
    f.seek(SeekFrom::Start(p0 + rmo)).unwrap();
    f.read_exact(&mut base_reg).unwrap();
    assert_eq!(&Sha256::digest(&base_reg)[..], &nh[0x1E0..0x200]);
    let mut blob = vec![0u8; rms as usize];
    f.seek(SeekFrom::Start(p0 + rmo)).unwrap();
    f.read_exact(&mut blob).unwrap();
    assert_eq!(&blob[0..8], b"IVFC\x00\x00\x01\x00");
    let sm = u32::from_le_bytes(blob[8..12].try_into().unwrap()) as u64;
    let (s1, l1) = (u64::from_le_bytes(blob[20..28].try_into().unwrap()), u32::from_le_bytes(blob[28..32].try_into().unwrap()));
    let (s2, l2) = (u64::from_le_bytes(blob[44..52].try_into().unwrap()), u32::from_le_bytes(blob[52..56].try_into().unwrap()));
    let (s3, l3) = (u64::from_le_bytes(blob[68..76].try_into().unwrap()), u32::from_le_bytes(blob[76..80].try_into().unwrap()));
    let al = |v: u64, o: u32| v.next_multiple_of(1u64 << o);
    let o3 = al(0x60 + sm, l3);
    let o1 = o3 + al(s3, l3);
    let o2 = o1 + al(s1, l1);
    // Cadena completa bottom-up.
    for (lvl_data, lvl_hash, ls, lo) in [(&blob[o3 as usize..], &blob[o2 as usize..], s3, l3), (&blob[o2 as usize..], &blob[o1 as usize..], s2, l2), (&blob[o1 as usize..], &blob[0x60..], s1, l1)] {
        let e = ls.next_multiple_of(1u64 << lo) >> lo;
        for i in 0..e {
            let a = (i << lo) as usize;
            let h = Sha256::digest(&lvl_data[a..a + (1usize << lo)]);
            assert_eq!(&h[..], &lvl_hash[(i as usize) * 0x20..(i as usize) * 0x20 + 0x20]);
        }
    }
    // Tablas: f1 @0 len 7 con bytes nuevos; f2 reempaquetado @7 intacto.
    let fdo = u32::from_le_bytes(blob[o3 as usize + 36..o3 as usize + 40].try_into().unwrap()) as u64;
    let fbase = o3 + fdo;
    let e1o = o3 + 0x28;
    let e1d = u64::from_le_bytes(blob[e1o as usize + 8..e1o as usize + 16].try_into().unwrap());
    let l1n = u64::from_le_bytes(blob[e1o as usize + 16..e1o as usize + 24].try_into().unwrap());
    assert_eq!((e1d, l1n), (0, 7));
    assert_eq!(&blob[fbase as usize..fbase as usize + 7], new_f1);
    let e2o = e1o + (0x20u64 + 4).next_multiple_of(4);
    let e2d = u64::from_le_bytes(blob[e2o as usize + 8..e2o as usize + 16].try_into().unwrap());
    let l2n = u64::from_le_bytes(blob[e2o as usize + 16..e2o as usize + 24].try_into().unwrap());
    assert_eq!((e2d, l2n), (7, 4));
    assert_eq!(&blob[fbase as usize + 7..fbase as usize + 11], b"1234");
    std::fs::remove_dir_all(&dir).ok();
}
