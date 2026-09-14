# Sidecars de xdelta3 (binarios oficiales, v3.2.0)

La GUI necesita el binario oficial `xdelta3` (decode VCDIFF con compresor
secundario LZMA). Se decidió no reimplementarlo: su correctitud la garantiza
el propio upstream y hay builds oficiales para los tres SO.

Origen (14-sep-2026, proyecto jmacd/xdelta, release v3.2.0):

- `linux-x86_64/xdelta3`   ← `xdelta3-3.2.0-linux-x86_64.tar.gz`
- `macos-arm64/xdelta3`     ← `xdelta3-3.2.0-macos-arm64.tar.gz`
- `windows-x86_64/xdelta3.exe` ← `xdelta3-3.2.0-windows-x86_64.zip`

SHA256 (para reverificar al actualizar):

- linux:   ejecutar `sha256sum sidecars/linux-x86_64/xdelta3`
- windows: `Get-FileHash sidecars\windows-x86_64\xdelta3.exe -Algorithm SHA256`
- mac:     `shasum -a 256 sidecars/macos-arm64/xdelta3`

Comprobado: `xdelta3 -V` dice 3.2.0 en los tres y el de Linux reproduce el
`printhdrs` esperado (359 ventanas, target total 3005760512).

## Licencia (Apache 2.0)

xdelta3 es Apache 2.0 (© Joshua MacDonald). Su §4 exige entregar el texto de
la licencia al redistribuir el binario: cada carpeta lleva su copia exacta
(`LICENSE`, tal cual del repo upstream, `xdelta3/LICENSE` — no hay NOTICE
que preservar). Al actualizar los binarios, renovar las tres copias.

Notas:

- No hay build oficial para macOS Intel (x86_64). En ese caso la app usa
  `xdelta3` del PATH si existe, o se compila desde el tarball de fuentes del
  mismo release (`xdelta3-3.2.0.tar.gz`) y se coloca en
  `sidecars/macos-x86_64/xdelta3` (la app ya busca ese directorio).
- En desarrollo la app acepta `IE_XDELTA3=/ruta/a/xdelta3` y, en última
  instancia, el `xdelta3` del PATH.
