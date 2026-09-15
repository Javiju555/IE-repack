// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Validación IE 1-2-3 (LOCAL, ignorado por defecto).
//!
//! Aplica la cadena publicada de Luis (v1 y luego v1.1-update, en estricto)
//! sobre un dump propio y comprueba los hashes intermedios/finales que él
//! publica. Es la prueba que confirma que el modo genérico sirve para su
//! traducción sin modificar nada.
//!
//! Requiere (nunca en el repo):
//!   IE123_BASE     = CCI japonés propio (p. ej. el de Vimm descifrado)
//!   IE123_PATCHDIR = dir con inazuma123-es-v1.xdelta e
//!                    inazuma123-es-v1.1-update.xdelta (repo de Luis)
//!   IE123_OUT      = directorio en disco real con ~10 GB libres
//!   xdelta3 en PATH o IE_XDELTA3
//!
//! Se ejecuta con: cargo test -p ie-core --test ie123_strict -- --ignored
//!
//! Estado 2026-09-15: con el dump Vimm/No-Intro (`35f74970...`) falla en el
//! paso 1 por diseño (224/256 ventanas coinciden; el dump es otra revisión
//! que la base del parche). Pasará cuando la base coincida. Ver NOTA_IE123.md.
//!
//! Hashes de referencia (docs de Luis):
//!   v1 intermedio: 73007ea1dc21e4a07a33f4038012ac66b03e644f89318b42a5d4997c79c6bbd5
//!   v1.1 final:    aa5a9f6c5da4a2a98eb5dde97ceba51fa350cbff6f406893d5e08d515b9fd1e9

use ie_core::pipeline;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

const V1_INTERMEDIO: &str =
    "73007ea1dc21e4a07a33f4038012ac66b03e644f89318b42a5d4997c79c6bbd5";
const V11_FINAL: &str =
    "aa5a9f6c5da4a2a98eb5dde97ceba51fa350cbff6f406893d5e08d515b9fd1e9";

#[test]
#[ignore]
fn cadena_ie123_v1_v11() {
    let base = PathBuf::from(std::env::var("IE123_BASE").expect("IE123_BASE"));
    let pdir = PathBuf::from(std::env::var("IE123_PATCHDIR").expect("IE123_PATCHDIR"));
    let outdir = PathBuf::from(std::env::var("IE123_OUT").expect("IE123_OUT"));
    let cancel = AtomicBool::new(false);
    let mut prog = |_: pipeline::StageProgress| {};

    // Paso 1: v1 sobre la base, con su hash intermedio como puerta.
    let v1out = outdir.join("ie123_v1.3ds");
    pipeline::run_strict(
        &pipeline::StrictInputs {
            base: base.clone(),
            patches: vec![pdir.join("inazuma123-es-v1.xdelta")],
            expected_sha256: Some(V1_INTERMEDIO.into()),
            expected_content_sha256: None,
            out_cci: v1out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut prog,
    )
    .unwrap();

    // Paso 2: v1.1-update sobre el resultado, con hash final.
    let v11out = outdir.join("ie123_v11.3ds");
    pipeline::run_strict(
        &pipeline::StrictInputs {
            base: v1out,
            patches: vec![pdir.join("inazuma123-es-v1.1-update.xdelta")],
            expected_sha256: Some(V11_FINAL.into()),
            expected_content_sha256: None,
            out_cci: v11out,
            keep_work: false,
        },
        &cancel,
        &mut prog,
    )
    .unwrap();
}
