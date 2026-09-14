# GUI de ie-galaxy-repack

Ver el `README.md` de la raíz (sección «Aplicación gráfica») para uso y
decisiones de diseño.

## Distribuir

Junto al binario (`ie-galaxy-repack` / `ie-galaxy-repack.exe`) hay que llevar
la carpeta `sidecars/` tal cual:

```text
ie-galaxy-repack[.exe]
sidecars/
  linux-x86_64/xdelta3
  macos-arm64/xdelta3
  macos-x86_64/xdelta3      (opcional, compilar desde fuentes si hace falta)
  windows-x86_64/xdelta3.exe
```

La app busca `xdelta3` en este orden: variable `IE_XDELTA3` → `sidecars/`
junto al ejecutable → `PATH`.

## Desarrollar

```bash
cargo test -p ie-core            # unitarios (rápidos, sin dumps)
cargo test -p ie-core -- --ignored  # dorados locales (ver ie-core/tests/)
cargo build -p ie-gui --release  # binario en target/release/
```

Los tests dorados necesitan el dump propio y ~25 GB; ver cabecera de
`crates/ie-core/tests/pipeline_golden.rs`. Nunca meter dumps en el repo.
