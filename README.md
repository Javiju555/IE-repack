# IE-repack

[![Release](https://img.shields.io/github/v/release/Javiju555/ie-repack)](https://github.com/Javiju555/ie-repack/releases)
[![CI](https://github.com/Javiju555/ie-repack/actions/workflows/ci.yml/badge.svg)](https://github.com/Javiju555/ie-repack/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Herramienta para reconstruir versiones traducidas y jugables de **Inazuma
Eleven (3DS)** a partir de tu propio dump japonés y el parche público,
sin depender de tener el archivo byte-exacto contra el que se generó el
parche oficial.

## El problema que resuelve

El parche de traducción al español de Galaxy (v1.0.5, por el equipo liderado
por Ale_z_plumber) se distribuye como un `.xdelta` — un diff binario que
compara el CIA japonés byte a byte contra una fuente concreta
(`Supernovabase.cia`) que el equipo tenía en su momento. Las instrucciones
oficiales piden solo "un CIA desencriptado del juego", sin más detalle de
dónde sacarlo. Buscando esa fuente concreta no la encontramos distribuida en
ningún sitio público (no significa que no exista en algún lado, solo que no
apareció en la búsqueda) — y el CIA japonés más estándar que circula hoy (el
que da cualquier sitio de ROMs al buscar el juego) **no es byte-idéntico** a
lo que sea que usara el equipo original — así que el parche falla con un
error de checksum, aunque el juego sea exactamente el mismo.

Verificado con datos: de las 358 ventanas del `.xdelta`, **0 coinciden**
contra el CIA "estándar" actual tal cual se descarga (empezando desde el
primer byte), y solo 41 de 358 aunque se descifre primero. La causa principal
no es que el juego sea distinto — el layout del contenido es el mismo — sino
dos cosas: (1) el NCCH de los dumps que circulan sigue cifrado (`Secure`)
mientras el parche se generó contra una base descifrada (`None`), así que
todo lo que el parche copia del origen sale mal; (2) el wrapper CIA
(ticket/TMD) es único por dump y nunca va a coincidir.

Comparando el RomFS completo del juego japonés (descifrado) contra una copia
ya traducida encontrada por separado, de **3491 archivos la traducción toca
el ExeFS (código) y 2 archivos** (`ie6_a.fa` y `ie6_b.fa`, los archivos de
Level-5 donde vive el texto). El resto de diferencias observadas entre esos
dos dumps concretos (unos pocos vídeos, voces sueltas, manual) son de versión
de dump, no de la traducción, y esta herramienta conserva en esos casos los
de tu propio dump.

Hay dos modos, según lo que tengas a mano. El primero usa directamente el
parche público; el segundo trabaja a nivel de ficheros RomFS:

```bash
# Con tu CIA japonés + el parche público (recomendado, no necesitas nada traducido):
./scripts/rebuild.sh --base tu_galaxy_japones.cia --xdelta-patch "parche_SN.xdelta" --out salida.3ds
# (también acepta el .zip tal cual se descarga del blog)
```

Cómo funciona el modo xdelta: descifra tu base in place (mismo layout,
`Secure` → `None`), aplica el parche en modo forzado (`xdelta3 -d -n`, los
checksums fallan por diseño según lo explicado arriba) y reempaqueta el CIA
para regenerar el TMD con los hashes correctos. Verificado byte a byte contra
una copia traducida de referencia: cabecera NCCH, IVFC, Level-3, ExeFS y ambos
`.fa` idénticos (solo quedan de tu dump las voces sueltas y el manual, que el
parche no toca).

**Sobre "funciona con cualquier ROM":** no es una garantía absoluta. Lo que
sí sabemos: el enfoque no depende de que el CIA de entrada coincida byte a
byte con nada concreto, así que debería funcionar
con cualquier dump japonés correctamente formado de Galaxy Supernova. Solo
lo hemos probado contra una fuente en concreto (la más estándar que
encontramos, la que da cualquier sitio de ROMs al buscar el juego) —
funcionó a la primera. No hemos probado con Big Bang ni con otras fuentes
distintas todavía.

## Uso

### Aplicación gráfica (recomendada si no usas la terminal)

1. Baja el `.zip`/`.tar.gz` de tu sistema desde
   [Releases](https://github.com/Javiju555/ie-repack/releases) —
   Windows, macOS (Apple Silicon) o Linux x86_64. Portable, sin instalador.
2. Descomprímelo entero (el ejecutable necesita la carpeta `sidecars/` al
   lado, no lo muevas suelto).
3. Abre el programa y elige qué quieres hacer en la pantalla de inicio:
   - **Galaxy Supernova**: tu CIA japonés + el parche (`.xdelta` o el
     `.zip` del blog tal cual).
   - **Pack de traducción**: tu base japonesa (`.cia`/`.3ds`) + la carpeta
     o el `.zip` del pack (con `manifiesto.json`). Cada fichero se
     verifica dos veces por SHA-256.
   - **Xdelta estricto** (experimental): base + cadena de parches en orden.
4. Elige dónde guardar y pulsa el botón. Hacen falta ~10–12 GB libres donde
   guardes la salida.

Nada de terminal, nada de instalar Rust ni nada más — el binario ya trae todo
lo que necesita. La app escribe siempre `ie-repack.log` junto al
ejecutable: si algo falla, adjúntalo al abrir un issue.

<details>
<summary>Compilar desde fuente (solo si quieres tocar el código)</summary>

```bash
cd gui
cargo build --release -p ie-gui
# El binario queda en gui/target/release/ie-repack
```

La app arranca en una pantalla de inicio con los tres modos explicados
(sin modo por defecto). El modo genérico "Parche .3ds (estricto,
experimental)": acepta base `.cia`/`.3ds`, una cadena
ordenada de parches y SHA final o de contenido opcionales (el de contenido,
desde el byte 0x200, es el que puede pasar una base convertida desde CIA,
cuyo wrapper NCSD difiere del cartucho por linaje). Sirve para parches
distribuidos sobre CCI descifrado canónico.

El modo pack acepta una carpeta o un `.zip` con `manifiesto.json` + parches
`.xdelta` por fichero: verifica el SHA original de cada fichero de tu
base, aplica su parche en estricto, verifica el resultado y reconstruye
el RomFS (vale aunque cambien los tamaños). Verificado de punta a punta
con IE 1-2-3 ES hasta el pack v3.0 (1234/1234 ficheros con su SHA-256
resultado comprobados en la imagen final), incluido arranque en español
en Azahar.

</details>

Decisiones (documentadas a propósito para quien retome esto):

- **Rust + egui** (binario único por SO, sin runtimes tipo WebView): `gui/` es
  un workspace Cargo con `ie-core` (toda la lógica: parse CIA, descifrado
  NCCH AES-CTR, construcción CCI, orquestación) e `ie-gui` (la ventana).
- **Híbrido nativo + un sidecar**: CIA/NCCH/CCI están reimplementados en
  `ie-core` y verificados byte a byte contra `ctrtool`/`3dstool`/`makerom`
  (tests dorados locales en `ie-core`, ignorados por defecto porque necesitan
  el dump propio). El decode VCDIFF lo hace el `xdelta3` oficial empaquetado
  en `gui/sidecars/` (v3.2.0, builds oficiales por SO), porque reimplementar
  su compresor secundario LZMA no compensaba.
- Las claves AES de 3DS que usa el descifrado son las mismas que distribuye
  3dstool en su fuente abierta (ver `gui/crates/ie-core/src/ncch.rs`).
- Todo el temporal va junto a la salida (disco real, nunca `/tmp` si es
  tmpfs) y hacen falta ~10 GB libres. Nada de ROMs/CIAs sale del disco del
  usuario ni entra al repo (ver `.gitignore`).

### Script (terminal)

Requiere `ctrtool`, `3dstool` y `makerom` instalados y en el PATH (en Arch:
`ctrtool` y `3dstool` están en repos oficiales/AUR, `makerom` vía AUR como
`projectctr-makerom-bin`). El modo `--xdelta-patch` necesita además `xdelta3`,
`python3` y ~20 GB libres donde vaya la salida (mueve unos 3 GB varias veces).
Si apuntas al `.zip` del blog en vez de al `.xdelta` suelto, también `unzip`.

```bash
# Si ya tienes ie6_a.fa / ie6_b.fa traducidos sueltos:
./scripts/rebuild.sh --base tu_galaxy_japones.cia --fa-dir /ruta/a/esos/fa --out salida.3ds

# Si lo que tienes es un CIA completo ya traducido:
./scripts/rebuild.sh --base tu_galaxy_japones.cia --translated-cia otro_ya_traducido.cia --out salida.3ds
```

Estos dos últimos modos no aplican el parche: desempaquetan el RomFS de tu
base, sustituyen solo los dos `.fa` y reempaquetan (conservan tu ExeFS).

El resultado es un `.3ds` (imagen de cartucho/CCI) descifrado. Ver
"Estado real y pendientes" para lo que hay y lo que no.

## Estado real y pendientes

Lo que **hoy, en `main`, hace y no hace** esta herramienta:

| | Estado |
|---|---|
| Salida `.3ds` descifrado | **Funciona**, en los tres modos |
| Verificación por SHA-256 de cada fichero del pack | **Funciona** |
| `.cia` de salida | **No está en `main`. Pendiente** (ver abajo) |
| Parcheo de ExeFS (banner/icon) desde el manifiesto | **No está en `main`. Pendiente** |
| Nombre de salida leído del SMDH | **No está en `main`. Pendiente** |
| Cifrado para flashcart | No previsto todavía |

Lo verificado hasta ahora, con números:

- **Pack v3.0 de Inazuma 1·2·3** (1234 ficheros): 1234/1234 con su SHA-256
  resultado comprobados **en la imagen final**, y arranque en español en
  emulador. En 3DS real, pendiente.
- **Pack v2.0**: instalado y jugado en Old XL real (11.17, Luma3DS).
- **Galaxy Supernova**: verificado byte a byte contra una copia de referencia.

### Pendiente: salida `.cia` instalable

**Objetivo:** que la herramienta produzca un `.cia` que se instale en una
3DS real con Luma3DS, no solo en emulador.

**Dónde está.** Se desarrolló y llegó a instalar correctamente en Azahar
(2.181.014.528 bytes, con `00000000.app` + `00000001.app` + TMD rehasheado),
pero **no se ha Integrado en `main`**: falta el interruptor de formato en la
GUI y su test. La pieza clave ya está entendida — el detalle que hace que un
instalador acepte el CIA es montar un **ticket falso al estilo GodMode9**
(firma y ECDSA a `0xFF`, titlekey a `0xFF`, derechos genéricos) y **rehashear
el TMD**, que es lo que los instaladores verifican.

> Esto **corrige una conclusión anterior que era falsa**. Durante un tiempo se
> anotó aquí que "Azahar rechaza instalar cualquier CIA sin firma real de
> verdad", con el error `d8a08004`. **No era eso.** No existe tal política en el
> emulador: era el writer antiguo, que no montaba el ticket ni rehasheaba el
> TMD. Con el writer correcto el mismo CIA instala.

**Lo que falta y no está resuelto.** En una Old XL con Luma3DS + GodMode9
**tampoco instala el CIA japonés de fábrica** — "instalación fallida", sin
detalle. Eso apunta a un problema **ajeno a esta herramienta**, y es
precisamente lo que bloquea el objetivo: no podemos validar nuestro `.cia` si
el punto de partida tampoco instala. Los `.cia` pequeños sí instalan en esa
misma consola, así que el CFW, los parches de firma y la SD están bien.

**Cómo lo vamos a probar**, de menos a más caro:

1. **Sin consola, ya se puede:** analizar por qué el CIA japonés de fábrica no
   es instalable — comparar su TMD, ticket y `contentinfo` contra lo que
   espera un instalador, y contra la salida de nuestro writer. Es trabajo de
   escritorio con las herramientas del repo.
2. **El mensaje literal del fallo:** el log de GodMode9 (`SD:/gm9/logs/`) o el
   error de FBI, que es más descriptivo. Sin el literal cualquier hipótesis
   es humo — es justo lo que hizo que una investigación anterior se colara por
   la SD.
3. **Control en consola:** instalar un `.cia` homebrew pequeño. Si entra, la
   vía de instalación está sana y el problema es de este contenido concreto.
4. **Bisección por tamaño:** construir un `.cia` con solo el contenido de la
   actualización (índice 1, pequeño) y ver si instala. Discrimina
   "problema de tamaño" de "problema de ticket/TMD".
5. **Solo entonces**, instalar nuestro `.cia` y arrancar desde el menú HOME.

```bash
# Equivalente a mano para pasar de CIA a CCI, por si vienes de la vía terminal:
makerom -ciatocci reempaquetado.cia -o reempaquetado.3ds
```

### Otras limitaciones

- No hay salida cifrada para flashcart. Sería la fase 2.
- Big Bang debería funcionar igual (mismo layout de CIA), pero no se ha probado.
- El parche de Luis solo cubre la **versión 1.0** del juego. Con la
  actualización oficial 1.4 instalada, el juego carga parte del código desde
  ella y el modo Pack **no la modifica**: hay que aplicarle además su parche.

## Licencia

MIT — ver [LICENSE](LICENSE). La app empaqueta `xdelta3` (Apache 2.0, con su
texto de licencia en `gui/sidecars/`) y el descifrado usa las mismas claves
AES que distribuye 3dstool en su fuente abierta. Este repositorio no incluye
ni incluirá ninguna ROM, CIA, ni archivo de datos del juego — solo el script
que automatiza la reconstrucción. Los créditos de la traducción son
enteramente del equipo original liderado por Ale_z_plumber.
