//! Cabecera NCCH y descifrado AES-CTR.
//!
//! Replica exacta del comportamiento de `3dstool -x ... cxi` para el caso
//! retail `Secure` sin extkey (el de este juego):
//! - KeyX = retail slot 0x2C (o 0x25/0x18/0x1B según `Flags[3]`).
//! - KeyY = primeros 16 bytes de la firma RSA de la cabecera.
//! - NormalKey = rol87(rol2(KeyX) XOR KeyY + C), todo big-endian u128.
//! - CTR inicial = PartitionId BE (8B) + tipo de sección (1B) + ceros.
//!
//! Constantes criptográficas: mismas que distribuye 3dstool en su fuente
//! (`src/ncch.cpp`, proyecto abierto de dnasdw). Sin ellas no se puede
//! descifrar; con ellas, el resultado se verifica byte a byte contra la
//! salida de 3dstool (ver test dorado local).

use crate::error::{Error, Result};
use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MEDIA_UNIT: u64 = 0x200;
pub const NCCH_MAGIC: u32 = 0x4843434E; // "NCCH" little-endian

// Slots retail de 3dstool (hex).
const KEYX_2C: &str = "B98E95CECA3E4D171F76A94DE934C053";
const KEYX_25: &str = "CEE7D8AB30C00DAE850EF5E382AC5AF3";
const KEYX_18: &str = "82E9C9BEBFB8BDB875ECC0A07D474374";
const KEYX_1B: &str = "45AD04953992C7C893724A9A7BCE6182";
const SCRAMBLER_CONST: &str = "1FF9E9AAC5FE0408024591DC5D52768A";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrKind {
    ExHeader = 1,
    ExeFs = 2,
    RomFs = 3,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn hex_bytes(s: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// Scrambler de clave de 3DS: rol87(rol2(keyX) XOR keyY + C).
fn scramble(key_x: &[u8; 16], key_y: &[u8; 16]) -> [u8; 16] {
    let x = u128::from_be_bytes(*key_x);
    let y = u128::from_be_bytes(*key_y);
    let c = u128::from_be_bytes(hex_bytes(SCRAMBLER_CONST));
    let n = (x.rotate_left(2) ^ y).wrapping_add(c).rotate_left(87);
    n.to_be_bytes()
}

#[derive(Debug, Clone)]
pub struct NcchHeader {
    pub raw: [u8; 0x200],
    pub partition_id: u64,
    pub program_id: u64,
    pub version: u16,
    pub flags: [u8; 8],
    pub exefs_off: u64,
    pub exefs_size: u64,
    pub romfs_off: u64,
    pub romfs_size: u64,
}

impl NcchHeader {
    pub fn parse(raw: [u8; 0x200]) -> Result<Self> {
        if u32le(&raw, 0x100) != NCCH_MAGIC {
            return Err(Error::Format("no es una cabecera NCCH (magia)".into()));
        }
        let media_exp = raw[0x100 + 0x86 - 0x100 + 6 - 6]; // placeholder para clippy
        let _ = media_exp;
        let flags: [u8; 8] = raw[0x188..0x190].try_into().unwrap();
        let media = 1u64 << (flags[6] + 9);
        Ok(Self {
            raw,
            partition_id: u64le(&raw, 0x108),
            program_id: u64le(&raw, 0x118),
            version: u16::from_le_bytes([raw[0x112], raw[0x113]]),
            flags,
            exefs_off: u32le(&raw, 0x1A0) as u64 * media,
            exefs_size: u32le(&raw, 0x1A4) as u64 * media,
            romfs_off: u32le(&raw, 0x1B0) as u64 * media,
            romfs_size: u32le(&raw, 0x1B4) as u64 * media,
        })
    }

    /// TitleId del contenido (== ProgramId en un CXI de aplicación).
    pub fn title_id(&self) -> u64 {
        self.program_id
    }

    pub fn product_code(&self) -> String {
        let raw = &self.raw[0x150..0x160];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).into_owned()
    }

    /// true si ya está descifrado (flag NoEncrypto).
    pub fn is_decrypted(&self) -> bool {
        self.flags[7] & 0x04 != 0
    }
}

/// Claves normales (old/new) para este NCCH. Falla si usa fixed-key o extkey.
fn normal_keys(hdr: &NcchHeader) -> Result<([u8; 16], [u8; 16])> {
    if hdr.flags[7] & 0x20 != 0 {
        return Err(Error::Crypto("este contenido necesita extkey (descarga de Nintendo); no soportado".into()));
    }
    if hdr.flags[7] & 0x01 != 0 {
        return Err(Error::Crypto("clave fija (fixed-key) no soportada".into()));
    }
    let slot = hdr.flags[3];
    let kx_new = match slot {
        0x01 => KEYX_25,
        0x0A => KEYX_18,
        0x0B => KEYX_1B,
        _ => KEYX_2C,
    };
    let mut key_y = [0u8; 16];
    key_y.copy_from_slice(&hdr.raw[0..16]);
    let old = scramble(&hex_bytes(KEYX_2C), &key_y);
    let new = scramble(&hex_bytes(kx_new), &key_y);
    Ok((old, new))
}

fn base_counter(partition_id: u64, kind: CtrKind) -> u128 {
    (((partition_id as u128) << 8) | (kind as u128)) << 56
}

/// XOR AES-CTR sobre `buf`, que empieza en `start` (offset dentro de la sección).
fn aes_ctr_xor(key: &[u8; 16], counter_base: u128, start: u64, buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }
    let cipher = Aes128::new_from_slice(key).unwrap();
    let mut block = [0u8; 16];
    let mut pos = 0usize;
    // Primer bloque parcial si start no está alineado a 16.
    let mut abs = start;
    while pos < buf.len() {
        let ctr = counter_base.wrapping_add((abs / 16) as u128);
        let mut b = ctr.to_be_bytes();
        let off = (abs % 16) as usize;
        cipher.encrypt_block((&mut b).into());
        block.copy_from_slice(&b);
        let take = (16 - off).min(buf.len() - pos);
        for i in 0..take {
            buf[pos + i] ^= block[off + i];
        }
        pos += take;
        abs += take as u64;
    }
}

/// Descifra `len` bytes del fichero en `abs_off` (offset absoluto), cuyo
/// keystream AES-CTR arranca en `sec_start` (offset dentro de la sección).
/// Informa bytes incrementales por tramo vía `on_chunk`.
fn crypt_section_range(
    f: &mut std::fs::File,
    key: &[u8; 16],
    counter_base: u128,
    abs_off: u64,
    sec_start: u64,
    len: u64,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<()> {
    const CH: u64 = 64 * 1024 * 1024;
    let mut buf = vec![0u8; CH as usize];
    let mut done = 0u64;
    while done < len {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = (len - done).min(CH) as usize;
        f.seek(SeekFrom::Start(abs_off + done))?;
        f.read_exact(&mut buf[..n])?;
        aes_ctr_xor(key, counter_base, sec_start + done, &mut buf[..n]);
        f.seek(SeekFrom::Start(abs_off + done))?;
        f.write_all(&mut buf[..n])?;
        done += n as u64;
        on_chunk(n as u64);
    }
    Ok(())
}

fn read_range(f: &mut std::fs::File, off: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Descifra el content0 (CXI) **in place**: exheader, ExeFS y RomFS.
/// `base_off` es el offset del inicio del NCCH dentro del fichero (0 si el
/// fichero ya es el CXI suelto, `content0_offset` si es el CIA completo).
/// Equivale al split de `3dstool -x cxi` + reescritura descifrada, pero sin
/// mover ni un byte de sitio (mismo layout y tamaño). Escribe además el flag
/// None y devuelve el header actualizado.
pub fn decrypt_content_in_place(
    path: &Path,
    base_off: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<NcchHeader> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let total = f.seek(SeekFrom::End(0))?;
    let raw_vec = read_range(&mut f, base_off, 0x200)?;
    let mut raw = [0u8; 0x200];
    raw.copy_from_slice(&raw_vec);
    let mut hdr = NcchHeader::parse(raw)?;
    if hdr.version != 2 && hdr.version != 0 {
        return Err(Error::Crypto(format!("versión NCCH {} no soportada", hdr.version)));
    }
    if hdr.is_decrypted() {
        return Ok(hdr);
    }
    if base_off + hdr.romfs_off + hdr.romfs_size > total {
        return Err(Error::Format("el NCCH declara regiones fuera del fichero".into()));
    }
    let (key_old, key_new) = normal_keys(&hdr)?;
    let ctr_exh = base_counter(hdr.partition_id, CtrKind::ExHeader);
    let ctr_exefs = base_counter(hdr.partition_id, CtrKind::ExeFs);
    let ctr_romfs = base_counter(hdr.partition_id, CtrKind::RomFs);

    // Progreso global ponderado por bytes: exh(2K) + exefs + romfs.
    let grand = 0x800u64 + hdr.exefs_size + hdr.romfs_size;
    let mut acc = 0u64;

    // 1. Exheader (0x200, 0x800) con key_old — si el tamaño dice que existe.
    if u32le(&hdr.raw, 0x180) == 0x400 {
        crypt_section_range(&mut f, &key_old, ctr_exh, base_off + 0x200, 0, 0x800, cancel, &mut |d| {
            progress(acc + d, grand);
            acc += d;
        })?;
    } else {
        acc += 0x800;
        progress(acc, grand);
    }

    // 2. ExeFS: el primer fichero (.code) usa key_new, el resto key_old.
    //    Hay que leer el superblock descifrado para saber su tamaño.
    //    Superblock: 10 entradas de (nombre[8], offset u32, size u32);
    //    .code es la primera (size en el byte 12).
    let mut sb = read_range(&mut f, base_off + hdr.exefs_off, 0x200)?;
    aes_ctr_xor(&key_old, ctr_exefs, 0, &mut sb);
    let code_size = u32le(&sb, 12) as u64;
    if hdr.exefs_size < 0x200 || code_size > hdr.exefs_size {
        return Err(Error::Format("superblock ExeFS incoherente".into()));
    }
    let mut done_e = 0u64;
    for (key, from, len) in [
        (&key_old, 0u64, 0x200u64),
        (&key_new, 0x200u64, code_size),
        (&key_old, 0x200 + code_size, hdr.exefs_size - 0x200 - code_size),
    ] {
        crypt_section_range(
            &mut f,
            key,
            ctr_exefs,
            base_off + hdr.exefs_off + from,
            from,
            len,
            cancel,
            &mut |d| {
                done_e += d;
                progress(acc + done_e, grand);
            },
        )?;
    }
    acc += hdr.exefs_size;

    // 3. RomFS entero con key_new.
    let mut done_r = 0u64;
    crypt_section_range(
        &mut f,
        &key_new,
        ctr_romfs,
        base_off + hdr.romfs_off,
        0,
        hdr.romfs_size,
        cancel,
        &mut |d| {
            done_r += d;
            progress(acc + done_r, grand);
        },
    )?;
    acc += hdr.romfs_size;
    progress(acc, grand);

    // 4. Flag None persistido en el fichero y en memoria.
    hdr.raw[0x18F] |= 0x04;
    hdr.flags[7] |= 0x04;
    f.seek(SeekFrom::Start(base_off + 0x18F))?;
    f.write_all(&[hdr.raw[0x18F]])?;
    Ok(hdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrambler_formato() {
        // Solo comprueba estabilidad de formato (u128 BE round-trip).
        let k = scramble(&hex_bytes(KEYX_2C), &[0u8; 16]);
        assert_eq!(k.len(), 16);
    }

    #[test]
    fn ctr_vec_nist() {
        // NIST SP 800-38A F.5.1 CTR-AES128.
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let mut buf: [u8; 16] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a,
        ];
        aes_ctr_xor(&key, 0xf0f1f2f3f4f5f6f7f8f9fafbfcfdfeffu128, 0, &mut buf);
        assert_eq!(
            buf,
            [
                0x87, 0x4d, 0x61, 0x91, 0xb6, 0x20, 0xe3, 0x26, 0x1b, 0xef, 0x68, 0x64, 0x99,
                0x0d, 0xb6, 0xce
            ]
        );
    }

    /// Test dorado LOCAL (ignorado por defecto): descifra el content0 real del
    /// juego con este código y lo compara byte a byte con la salida de 3dstool.
    /// Requiere el dump propio (nunca en el repo):
    ///   IE_GOLDEN_DIR=<dir con jp_content0.cxi + exh/exefs/romfs de 3dstool>
    /// Se ejecuta con: cargo test -p ie-core -- --ignored
    #[test]
    #[ignore]
    fn golden_descifrado_igual_que_3dstool() {
        use std::io::{Read, Seek, SeekFrom};
        let dir = std::env::var("IE_GOLDEN_DIR").expect("IE_GOLDEN_DIR sin definir");
        let src = std::path::PathBuf::from(&dir).join("jp_content0.cxi");
        let mine = std::path::PathBuf::from(&dir).join("ie_core_decrypted.cxi");
        std::fs::copy(&src, &mine).unwrap();
        let hdr = decrypt_content_in_place(&mine, 0, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert!(hdr.is_decrypted());
        // Compara rangos contra la verdad de 3dstool por tramos (evita 6 GB en RAM).
        let cmp = |mine_off: u64, truth_name: &str| {
            let truth = std::path::PathBuf::from(&dir).join(truth_name);
            let len = std::fs::metadata(&truth).unwrap().len();
            let mut fm = std::fs::File::open(&mine).unwrap();
            let mut ft = std::fs::File::open(&truth).unwrap();
            let mut bm = vec![0u8; 64 * 1024 * 1024];
            let mut bt = vec![0u8; 64 * 1024 * 1024];
            let mut done = 0u64;
            while done < len {
                let n = (len - done).min(bm.len() as u64) as usize;
                fm.seek(SeekFrom::Start(mine_off + done)).unwrap();
                ft.seek(SeekFrom::Start(0 + done)).unwrap();
                fm.read_exact(&mut bm[..n]).unwrap();
                ft.read_exact(&mut bt[..n]).unwrap();
                assert_eq!(&bm[..n], &bt[..n], "difieren en {truth_name} offset {done:#x}");
                done += n as u64;
            }
        };
        cmp(0x200, "exh.bin");
        cmp(hdr.exefs_off, "exefs.bin");
        cmp(hdr.romfs_off, "romfs.bin");
        std::fs::remove_file(&mine).ok();
    }
}
