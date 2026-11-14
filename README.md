# VistaForge

**Générateur et éditeur de paysages 2.5D moderne**, écrit en Rust 2024.
Terrain fractal ou DEM réel, éclairage solaire, ciel et nuages procéduraux, eau, rendu PNG déterministe,
projet `.vfproj`, CLI, interface de bureau et serveur MCP.

![Canyon avec falaises généré par VistaForge](docs/screenshots/canyon_cliffs.png)

## Ce qui fonctionne

- **Terrain** : génération Classic (midpoint displacement, seed, dimension fractale, diviseur, île) et Modern
  (fBm, ridged multifractal, domain warp, presets, cratères, terrasses, bordure insulaire).
- **Détail continu** : champ fractal multi-échelle évalué au-dessus du DEM, band-limité par l'empreinte pixel,
  profils granite/basalte/calcaire/grès/schiste/craie/volcanique/canyon/Patagonie, patch de gros plan
  à 2–50 cm, sillons, arêtes, cratères, exfoliation, vésicules. Maillage subdivisé 1–4×,
  **adaptatif par blocs de 64 cellules** avec jupes de raccord, budget de 6 M de sommets.
- **Édition** : érosion hydraulique + thermique, lissage, ré-échantillonnage bicubique, brosses,
  normalisation, statistiques, masque de validité.
- **Modificateurs** : pile live de 27 opérateurs — Displace, Warp, Transform, Swirl, Terrace,
  Fold, Stratify, Grooves, Ridges, Crater field, Exfoliate, Vesicles, Undercut, Aperture,
  Clamp, Blur, Sharpen, Crater, Recast cliff, Erosion, Thermal, Alluvium, Snow, Dunes, Talus,
  Combine, BiasGain — appliquée au rendu, éditable au CLI, dans l'UI et via MCP, avec cuisson
  (`bake`).
- **Géologie** : modèle de roche procédural — strates pendées avec dureté par banc, joints
  orthogonaux et colonnes basaltiques, dissolution karstique, détail de surface. Presets
  `granite`, `basalt`, `limestone`, `sandstone`, `schist`, `chalk`, `volcanic`, appliqués avant
  l'érosion et relus par le shader pour colorer les bancs.
- **DEM** : lecture/écriture du format DEM legacy v1, import, export, round-trip sémantique,
  palette préservée, session opaque.
- **Scène** : caméra look-at/orbite, ciel crépusculaire avec étoiles, canyon creusé,
  soleil (azimut boussole 0 = sud, 90 = ouest), palettes
  alpine/désert/volcanique/côtier/canyon/lunaire/île, brume, exposition, contraste, saturation.
- **Rendu CPU déterministe** : rasterisation tuilée multi-thread, z-buffer, PCF 5×5, AO
  multi-échelle à courbe HBAO, ombres 4096 en qualité High, parois verticales (`cliffs`), ciel
  dégradé + soleil + nuages 2.5D, eau avec Fresnel et écume, brume, tone mapping ACES,
  supersampling et dithering ordonné.
- **Matériaux** : éboulis, neige en plaques, végétation, humidité, normales de détail.
- **Grands mondes** : heightfield jusqu'à 16 384², tuilage N×M avec flips/rotations,
  mode **seamless** (correction des quatre bords + variation macro), et modes de rendu
  `fast` / `refined` / `photoreal`. Voir [`docs/MONDE.md`](docs/MONDE.md).
- **Falaises** : pass `cliffs` (ladder de hauteurs à poids lissé) et modificateur `recast_cliff`
  (Poisson/SOR) pour des parois verticales continues.
- **Vol** : caméra FPV/hélicoptère déterministe (gîte physique, inertie, secousses),
  trajectoires orbite/circuit/crête/piqué, séquences PNG avec flou de mouvement 180°.
- **Rig caméra** : point de vue Bézier éditable, cible fixe/avance/courbe, caméra
  rigide ou ressort, FOV/DOF/roulis, presets drone/oiseau/hélicoptère/dirigeable/chasseur/
  manuel avec accélérations par axe, jerk, roulis en virage, contrainte de hauteur,
  sol de référence et évitement des parois par champ de distance. Éditeur graphique dans
  l'espace **Animation**. Voir [`docs/RIG.md`](docs/RIG.md).
- **Formats** : `.vfproj` (ZIP + RON + heightmap f32, schéma **v6**), PNG, ESRI ASCII Grid, DEM.
- **Bridge Blender** : export GLB (maillage détaillé, UV0 planaire + UV1 atlas de boîte,
  couleurs de sommets, 7 matériaux, caméra, soleil), sidecar JSON, textures cuites
  (albédo, normale, masques, heightfield 16 bits), script d'import, Smart UV/remesh optionnels
  et rendu Cycles/OptiX headless. Voir [`docs/BLENDER.md`](docs/BLENDER.md).
- **Interfaces** : CLI `vistaforge`, GUI `vistaforge-app`, serveur MCP stdio `vistaforge-mcp`
  (29 outils, confirmation des chemins au workspace, aperçus renvoyés en image MCP).
- **Qualité** : 152 tests, `cargo clippy -D warnings`, CI Ubuntu 24.04 + Windows.

| Interface | Aperçu |
|---|---|
| Bureau `egui` | ![Interface VistaForge](docs/screenshots/gui.png) |
| Canyon, falaises et strates | ![Canyon](docs/screenshots/canyon_cliffs.png) |
| Patagonie, tours verticales | ![Patagonie](docs/screenshots/patagonia.png) |
| Crépuscule étoilé | ![Crépuscule](docs/screenshots/dusk.png) |

## Démarrage rapide

Prérequis : rustup (toolchain épinglée installée automatiquement). Sous Windows, Build Tools MSVC.
Détails : [`docs/BUILD.md`](docs/BUILD.md).

```bash
cargo test --workspace

# Générer une île 1026² avec érosion et la rendre en 1080p
cargo run --release -p vf-cli -- terrain generate \
  --mode classic --size 1026 --seed 42 --island --erode \
  --name "Mon île" --out ile.vfproj

cargo run --release -p vf-cli -- render ile.vfproj ile.png \
  --quality high --width 1920 --height 1080

# Interface de bureau
cargo run --release -p vf-app

# Serveur MCP
cargo run --release -p vf-mcp -- --workspace .
```

Le CLI accepte aussi `terrain import` (DEM/ASCII), `terrain rock` (géologie sur un DEM),
`terrain resample`, `terrain smooth`, `terrain modifier ...`, `terrain sculpt`,
`terrain world`, `rig preset/info/key`, `anim still/render`, `blender export/render`,
`scene preset`, `project info`, `dem info/roundtrip/preview/convert`.
La référence complète est dans [`docs/USAGE.md`](docs/USAGE.md), les modificateurs dans
[`docs/MODIFICATEURS.md`](docs/MODIFICATEURS.md).

## Workspace

```
crates/vf-core      données : Seed, HeightField, caméra, soleil, palette, Project
crates/vf-terrain   resampling, brosses, normales, courbure, AO, hillshade, mondes
crates/vf-procgen   bruit, Classic, Modern, érosion, falaises
crates/vf-scene     cadrage caméra, orbite, auto-frame
crates/vf-io        DEM, .vfproj, PNG, ASCII Grid
crates/vf-geology   strates, joints, karst, détail de surface
crates/vf-modifiers 27 opérateurs de terrain, pile live
crates/vf-flight    trajectoires, dynamique de vol, rig caméra, clearance
crates/vf-detail    champ de détail continu par roche
crates/vf-mesh      maillage subdivisé, LOD adaptatif, patch de gros plan
crates/vf-export    GLB, textures cuites et bridge Blender
crates/vf-render    rasteriseur CPU, ombres, ciel, eau, matériaux, post-traitement
crates/vf-cli       binaire `vistaforge`
crates/vf-mcp       serveur MCP stdio + binaire `vistaforge-mcp`
crates/vf-ui        interface `egui`
crates/vf-app       binaire `vistaforge-app`
```

Règle de dépendance : `vf-core` ne dépend d'aucun crate lourd ; le rendu ne dépend pas de l'UI ;
le CLI et le MCP partagent le même noyau que l'interface.

## Documentation

| Fichier | Contenu |
|---|---|
| [`docs/USAGE.md`](docs/USAGE.md) | CLI, rendu, projets, MCP, interface, limites |
| [`docs/MODIFICATEURS.md`](docs/MODIFICATEURS.md) | les 27 modificateurs, paramètres, CLI/UI/MCP |
| [`docs/QUALITE_ECARTS.md`](docs/QUALITE_ECARTS.md) | analyse des écarts, plan qualité, essais négatifs |
| [`docs/ETAT_ET_REPRISE.md`](docs/ETAT_ET_REPRISE.md) | état du projet et reprise |
| [`docs/SPEC_REALISME.md`](docs/SPEC_REALISME.md) | contrat et faisabilité de la session réalisme |
| [`docs/RIG.md`](docs/RIG.md) | rig caméra, presets de dynamique, contraintes, CLI |
| [`docs/MONDE.md`](docs/MONDE.md) | tailles, tuilage, seamless, modes de rendu, évitement |
| [`docs/BLENDER.md`](docs/BLENDER.md) | bridge Blender, GLB, textures cuites, rendu Cycles |
| [`docs/BUILD.md`](docs/BUILD.md) | build Ubuntu/Windows, release, dépannage |
| [`docs/PLAN_ADDITIONS.md`](docs/PLAN_ADDITIONS.md) | audit 2026, veille, modèle géologique, module de vol, UI en barres |

## État et suite

Cette version 0.2 couvre les phases 0–1 du plan, l'essentiel de la phase 2 (matériaux par bandes,
ciel, eau, nuages 2.5D), le cœur MCP de la phase 5 et une UI de travail. Restent notamment :
les waypoints de vol libres, la végétation instanciée, le mode 8 bits, les nuages volumétriques,
le path tracing et le packaging. Le bridge Blender est en place (export aller) ; le retour
Blender → VistaForge n'est pas prévu.

## Légal

Code sous double licence **MIT OR Apache-2.0** (voir [`LICENSE-MIT`](LICENSE-MIT) et
[`LICENSE-APACHE`](LICENSE-APACHE)). Projet clean-room : les formats de fichiers sont reproduits
pour l'interopérabilité ; aucun asset, manuel ou code d'un logiciel tiers n'est distribué.

Une partie des idées d'algorithmes (recast cliff, talus, strates gamma, dune asymétrique,
HBAO, PCF) a été étudiée dans [Hesiod](https://github.com/ottolink-dev/Hesiod) et son noyau
[HighMap](https://github.com/ottolink-dev/HighMap), publiés sous GPLv3. Les implémentations
Rust de VistaForge sont indépendantes : aucune ligne de code GPL n'est copiée, et le projet
reste sous MIT OR Apache-2.0. Seules les idées mathématiques sont réutilisées.
