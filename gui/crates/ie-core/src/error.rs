// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("formato no reconocido: {0}")]
    Format(String),
    #[error("cifrado no soportado: {0}")]
    Crypto(String),
    #[error("xdelta3 falló: {0}")]
    Xdelta(String),
    #[error("cancelado por el usuario")]
    Cancelled,
    #[error("verificación fallida: {0}")]
    Verify(String),
    #[error("zip: {0}")]
    Zip(String),
}

pub type Result<T> = std::result::Result<T, Error>;
