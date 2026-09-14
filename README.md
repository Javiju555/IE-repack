# ie-galaxy-repack

Herramienta para reconstruir una versión traducida y jugable de **Inazuma
Eleven GO Galaxy** a partir de cualquier dump japonés correcto del juego,
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
byte con nada concreto (a diferencia del xdelta), así que debería funcionar
con cualquier dump japonés correctamente formado de Galaxy Supernova. Solo
lo hemos probado contra una fuente en concreto (la más estándar que
encontramos, la que da cualquier sitio de ROMs al buscar el juego) —
funcionó a la primera. No hemos probado con Big Bang ni con otras fuentes
distintas todavía.

## Uso

### Aplicación gráfica (recomendada si no usas la terminal)

En `gui/` hay una app de escritorio (Windows/Mac/Linux) que hace lo mismo que
el modo `--xdelta-patch` del script: eliges tu CIA japonés y el parche
(`.xdelta` o el `.zip` del blog), eliges dónde guardar y pulsas el botón, con
barra de progreso y registro en la ventana. Si pones los dos archivos junto al
ejecutable, los precarga solos al arrancar.

```bash
cd gui
cargo build --release -p ie-gui
# El binario queda en gui/target/release/ie-galaxy-repack
```

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
  tmpfs) y hacen falta ~15 GB libres. Nada de ROMs/CIAs sale del disco del
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

El resultado es un `.3ds` (imagen de cartucho/CCI), no un `.cia` — ver la
siguiente sección de por qué.

## Hallazgo aparte: Azahar rechaza instalar CIAs sin firma real

Esto no tiene que ver con la traducción en sí, pero vale la pena dejarlo
anotado porque cualquiera reempaquetando contenido de 3DS para Azahar se lo
va a encontrar: en builds recientes de Azahar (probado en 2126.0), **el
instalador de CIA rechaza cualquier CIA que no tenga una firma válida de
verdad** — no solo los reempaquetados por esta herramienta, sino también
dumps japoneses "normales" tal cual circulan por sitios de ROMs:

```
Service.AM <Error> Blocked unauthorized encrypted CIA installation.
Service.AM <Error> CIA file installation aborted with error code d8a08004
```

Probado también forzando la firma del ticket a cero (el patrón que usan
la mayoría de dumps de la escena) — mismo resultado. No parece ser un
problema de la firma en sí, sino del flujo de instalación en general para
contenido no firmado oficialmente.

La salida: **convertir a CCI (`.3ds`) en vez de distribuir como CIA**. Un
`.3ds` se carga directo con Archivo → Cargar archivo, sin pasar por ningún
instalador ni chequeo de autorización — es el mismo camino que seguiría un
cartucho real insertado en la consola. Confirmado arrancando correctamente,
con la pantalla de créditos de la traducción incluida.

```bash
makerom -ciatocci reempaquetado.cia -o reempaquetado.3ds
```

## Limitaciones actuales

- El modo `--xdelta-patch` es el recomendado y solo necesita tu CIA japonés +
  el parche público. Los modos `--translated-cia`/`--fa-dir` siguen ahí como
  alternativa sin parche (necesitan el contenido traducido completo).
- Probado específicamente contra Galaxy Supernova. Big Bang debería
  funcionar igual (mismo layout de CIA), pero no se ha probado.
- Si algún día se quiere un parche de verdad pequeño, haría falta entender
  el formato interno de `ie6_a.fa`/`ie6_b.fa` (archivos "fat archive" de
  Level-5) para diferenciar solo las entradas de texto en vez de mover el
  archivo completo.

## Licencia

MIT — ver [LICENSE](LICENSE). La app empaqueta `xdelta3` (Apache 2.0, con su
texto de licencia en `gui/sidecars/`) y el descifrado usa las mismas claves
AES que distribuye 3dstool en su fuente abierta. Este repositorio no incluye
ni incluirá ninguna ROM, CIA, ni archivo de datos del juego — solo el script
que automatiza la reconstrucción. Los créditos de la traducción son
enteramente del equipo original liderado por Ale_z_plumber.
