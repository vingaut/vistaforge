# État du projet et reprise

> Document de reprise des sessions des **13 et 14 novembre 2026**.
> Objectif : reprendre le travail sans relire l'historique, sur cette machine ou
> une autre. Il décrit ce qui fonctionne, ce qui a été ajouté, ce qui reste à
> faire et les pièges déjà rencontrés.

Dernière mise à jour : **2026-11-14 06:40** (sessions P0–P6 : détail, sculpteur, rig, mondes, Blender).

---

## 0ter. Session du 14 novembre — détail continu, maillage, régularité (P0)

- **Nouveau crate `vf-detail`** : champ de détail continu évalué au-dessus du
  heightfield. Fractal multi-échelle band-limité par l'empreinte pixel,
  sillons, arêtes, cratères, exfoliation, vésicules. Chaque couche a sa propre
  graine dérivée, ses coordonnées sont warpées et son échelle/amplitude varie
  avec un champ de patchiness basse fréquence (`heterogeneity`).
- **Nouveau crate `vf-mesh`** : maillage régulier subdivisé (1× à 8×) avec
  budget de sommets, normales issues du champ continu, UV, et **patch de
  gros plan** (`build_focus_patch`) à 2–50 cm de pas, bord fondu vers la
  surface de base. Le renderer découpe un trou dans le maillage grossier sous
  le patch pour éviter le z-fighting.
- **Renderer** : `subdivision` et `detail_geometry` dans `RenderOptions`,
  maillage construit par `vf-mesh`, AO échantillonnée au maillage, ombres et
  eau inchangées. `RenderSettings.subdivision` (1..4) et `cliff_warp`
  (0..1, échelle en mètres) ajoutés.
- **Régularité des falaises** : le ladder global est décalé par un champ
  basse fréquence appliqué **après** le choix du barreau (pas avant), pour
  onduler les couches sans créer d'aiguilles d'une cellule. Les couches de
  détail ont désormais des échelles, des angles et des densités variables.
- **Schéma projet v6** : champ `detail` (défaut désactivé, migration douce).
- **CLI** : `--detail-profile`, `--detail`, `--no-detail`, `--detail-strength`,
  `--detail-scale`, `--detail-amplitude`, `--detail-heterogeneity`,
  `--detail-focus`, `--detail-focus-size`, `--detail-focus-spacing`,
  `--detail-normal`, `--cliff-warp`, `--cliff-warp-scale`, `--subdivision`.
- **UI** : nouvel espace **Détail** (profil, subdivision, fractal, sillons,
  arêtes, cratères, exfoliation, vésicules, patch de gros plan).
- **Tests** : 122 tests verts, clippy `-D warnings` propre.
- **Démos** : `demo-out/detail/rock_comparaison.png` (avant/après
  régularité), `rock_before.png`, `rock_after.png`.
- **Limites** : les surplombs vrais, les arches et les colonnes basaltiques
  demandent la couche maillée locale (P1) ; le détail vertical est atténué sur
  les faces quasi verticales pour ne pas produire d'aiguilles.

---

## 0quater. Session du 14 novembre — micro-opérateurs et profils (P1)

- **Six opérateurs** ajoutés à la pile (`vf-modifiers::micro`) : `grooves`
  (sillons), `ridges` (arêtes), `crater_field` (cratères avec bourrelet et
  éjecta), `exfoliate` (feuillets), `vesicles` (puits) et `undercut` (niche
  sous un banc dur, approximation heightfield de la corniche). Tous sont
  déterministes, conscients de la résolution et paramétrables via `ParamSpec`.
- **Profil de détail automatique** : `terrain generate --rock X` et
  `terrain rock --preset X` appliquent le profil `vf-detail` correspondant
  (`granite`, `canyon`, `patagonia`, etc.). `--detail-profile` force un autre
  profil, `--no-detail` le désactive. L'UI Roche et MCP `geology.set` font de
  même.
- **UI** : groupe « Détail de roche » dans le sélecteur de modificateurs ;
  espace Détail (P0) déjà en place.
- **Docs** : `MODIFICATEURS.md` (27 opérateurs), README, `USAGE.md` (schéma
  v6), `project info` affiche détail, subdivision et cliff warp.
- **Tests** : 126 verts, clippy `-D warnings` propre.
- **Démo** : `demo-out/micro/gallery.png` (base + six opérateurs, rendu large).
- **Limite** : `undercut` creuse une niche ; le surplomb maillé vrai (lèvre
  qui dépasse) reste à faire au lot P1.5, avec les arches et les cheminées.

---

## 0quinquies. Session du 14 novembre — bridge Blender (P2)

- **Nouveau crate `vf-export`** : écrit un **GLB 2.0** à la main (pas de crate
  glTF), avec conversion Z-up → Y-up, positions/normales/UV0/UV1, couleurs de
  sommets relatives aux matériaux, 7 emplacements (roche, deux strates,
  éboulis, neige, végétation, eau), caméra, soleil `KHR_lights_punctual`.
- **Pipeline fidèle** : l'export applique la pile de modificateurs puis le
  pass falaises (`cliff_warp` inclus) avant de construire le maillage détail.
  Le pass falaises a été déplacé de `vf-render/cliff.rs` vers
  `vf-procgen/cliff.rs` pour être partagé sans dupliquer la logique.
- **Textures cuites** : `albedo.png` (sRGB, UV0), `normal.png` (normale
  dérivée du heightfield détaillé), `masks.png` (R=neige, G=végétation,
  B=éboulis), `heightmap_16.png` (16 bits). Sidecar `scene.json` avec palette,
  soleil, caméra, unités, stats, roche, détail, matériaux.
- **Script Blender** : `bridge/blender/vistaforge_import.py` importe le GLB,
  branche albédo/normale/masques, ajoute un bruit procédural de secours, règle
  AgX, le monde, le soleil, la caméra (FOV/DOF), Cycles CPU ou OptiX
  (`--gpu`), Smart UV et remesh optionnels, rendu headless.
- **CLI** : `vistaforge blender export <projet> <dossier>` et
  `vistaforge blender render <projet> <image> --samples N [--gpu]`.
- **Validation réelle** : Blender 5.3, import de 1 054 733 sommets, 24
  échantillons Cycles 960×600 en ~13 s ; image
  `docs/screenshots/blender_cycles.png`.
- **Tests** : 130 verts, clippy `-D warnings` propre.
- **Limites** : export aller seulement, pas de round-trip ; textures de départ
  non PBR multi-UDIM ; pas encore de végétation instanciée ; le patch de gros
  plan (`focus_enabled`) n'est pas fusionné dans l'export.

---

## 0sexies. Session du 14 novembre — sculpteur manuel (P3)

- **Moteur de pinceau** (`vf-terrain::brush`) : formes disque/carré/losange/
  anneau/étoile/polygone, retombées constante/linéaire/lisse/gaussienne/
  plateau, taille en mètres, aspect, angle, dureté, rugosité par bruit de
  valeur, anti-aliasing 1–8×, symétrie X/Y/XY/radiale, inversion.
- **Modes** : élever, abaisser, lisser, aplatir, terrasse, crête, sillon,
  cratère, bruit fractal, warp, cloner.
- **Trait** : `Stroke` interpole les stamps le long du segment, garde une
  copie de pré-trait et produit un `BrushUndo` limité à la région touchée
  (avant/après), avec undo/redo.
- **UI** : nouvel espace **Sculpter** — hillshade de dessus, zoom/pan, curseur
  de pinceau, panneau complet, presets Roche/Détail fin/Érosion/Cratère,
  historique et bouton de recalcul du hillshade.
- **CLI** : `terrain sculpt <projet> --x --y [--x2 --y2] --mode --shape ...
  --out` ; sortie JSON avec région et octets d'undo.
- **Démo** : `demo-out/sculpt/comparaison.png` (avant/après d'un trait étoile).
- **Tests** : 137 verts, clippy `-D warnings` propre.
- **Limite** : le hillshade de l'éditeur se met à jour après un trait ou un
  undo ; après un modificateur, utiliser « Recalculer le hillshade ». Pas
  encore de pression de tablette ni de calques de masque.

---

## 0septies. Session du 14 novembre — rig caméra (P4)

- **Modèle `CameraRig`** (schéma v6) : point de vue `Path3D` (clés + poignées
  Bézier + ease), cible fixe / avance / courbe / manuelle, optique (FOV,
  near/far, DOF, fstop, lien rigide ou ressort, roulis explicite), corps
  (`MotionProfile`), contraintes (hauteur sol, sol de référence, parois,
  douceur).
- **Presets** : drone, oiseau, hélicoptère, dirigeable, chasseur, manuel avec
  accélérations avant/latérale/verticale, jerk, traînée, gain et roulis max.
- **Simulation** (`vf-flight::rig`) : ressort critique sur la courbe,
  accélération bornée par axe, jerk limité, roulis `atan(a_lat/g)`, secousses
  déterministes, garde au sol, caméra ressort, DOF calculé. Le recul de
  timeline réinitialise proprement.
- **CLI** : `rig info`, `rig preset`, `rig key`, `anim still` et `anim render`
  utilisent le rig s'il est actif. Une séquence de 4 images 320×200 a été
  rendue.
- **UI** : espace **Animation** avec hillshade, courbe eye orange, courbe
  target bleue, clés déplaçables, panneau complet (presets, clés, optique,
  dynamique, contraintes) et aperçu au temps courant.
- **Docs** : `docs/RIG.md`, README, USAGE.
- **Tests** : à vérifier après le lot ; clippy `-D warnings` propre.
- **Limites** : flou de mouvement pas encore branché sur le rig ; évitement de
  parois et DOF rasterisé au lot P5.

---

## 0octies. Session du 14 novembre — parois, mondes, rendu (P5)

- **Évitement des parois** : `vf-flight::clearance` construit un champ de
  distance (chamfer) aux cellules de pente > 45° ou marches d'une cellule.
  Le rig échantillonne devant le point de vue et applique une accélération
  latérale répulsive sous `wall_distance_m`. `RigSample.wall_distance_m` est
  renseigné.
- **Mondes** : `WorldSettings` (schéma v6) avec `single`, `tiled`, `seamless`.
  `vf-terrain::world::compose` replique la tuile avec flips/rotations,
  variation d'échelle/offset en mode tuilé et champ macro continu.
  `make_seamless` corrige les quatre bords par rampe localisée (C0).
  CLI `terrain world --mode ... --tiles-x ... --apply`.
- **Tailles** : heightfield jusqu'à 16 384², avertissement mémoire au-delà de
  8 192. Le renderer coarsit le maillage au budget de 6 M de sommets.
- **Modes de rendu** : `RenderMode::{Fast, Refined, Photoreal}` avec presets
  SSAA/ombres/AO/subdivision ; CLI `rendu --mode`, combo UI.
- **UI** : panneau « Monde et tuilage » dans Terrain (composition et
  remplacement du terrain), combo de mode de rendu.
- **Démos** : `demo-out/world/seamless3x3.png`, `seamless3x3_macro.png`.
- **Tests** : 145+ verts, clippy `-D warnings` propre.
- **Limites** : seamless C0 (pas de correction de pente), pas de Wang,
  recomposer un projet déjà composé tuile le monde ; DOF calculé mais non
  rasterisé côté CPU (Blender le reçoit).

---

## 0nonies. Session du 14 novembre — UV, remesh, finition (P6)

- **UV1 atlas de boîte** : six projections packées en 3×2, exportées comme
  seconde couche UV du GLB. UV0 reste la projection planaire des textures
  cuites. Blender importe les deux (`2 UV layer(s)`).
- **Options Blender** : `--smart-uv`, `--remesh` (Voxel), `--save-blend` dans
  `blender render`; `--validate` pour un import sans rendu.
- **Validation** : import Blender 5.3 de 1 054 733 sommets, 2 matériaux,
  2 couches UV ; rendu Cycles 960×600 en ~13 s.
- **Démos** : `docs/screenshots/blender_cycles.png`,
  `docs/screenshots/world_seamless.png`, `docs/screenshots/rig_helicopter.png`,
  `docs/screenshots/detail_rock.png`.
- **Tests** : 151 verts, clippy `-D warnings` propre.
- **Limite** : l'atlas UV1 est par sommet ; les triangles qui changent d'axe
  peuvent montrer une couture. Pas de baking multi-UDIM ni de remesh
  VistaForge (le remesh est fait dans Blender).

---

## 0decies. Bilan de la session P0–P6 (14 novembre 2026)

| Lot | Livré | Limite assumée |
|---|---|---|
| P0 | Champ de détail continu multi-échelle (`vf-detail`), maillage régulier subdivisé + **LOD adaptatif par blocs avec jupes** (`vf-mesh`), patch de gros plan, intégration renderer, UI Détail | Le LOD adaptatif ne fait gagner des triangles que sur les grandes grilles (> 2 M d'échantillons) ; en dessous, la grille uniforme est plus rapide. |
| P1 | 6 opérateurs `grooves/ridges/crater_field/exfoliate/vesicles/undercut`, 9 profils de roche, application auto avec `--rock`, profils par couche, hétérogénéité | `undercut` creuse une niche heightfield ; le vrai surplomb maillé reste à faire. |
| P2 | `vf-export` : GLB 2.0, UV0/UV1, couleurs, 7 matériaux, caméra, soleil, sidecar, textures cuites, script Blender, rendu Cycles | Export aller seulement ; textures de départ non PBR multi-UDIM. |
| P3 | Moteur de pinceau complet, formes, fractal, AA, symétrie, undo par région, UI Sculpter, CLI `terrain sculpt` | Pas de pression tablette ; hillshade à recalculer après un modificateur. |
| P4 | Rig LightWave (eye/target/optics), Bezier éditable, presets drone/oiseau/hélico/dirigeable/chasseur/manuel, accélérations par axe, jerk, roulis, FOV/DOF, ressort caméra, UI Animation, CLI `rig` + `anim` | DOF calculé mais non rasterisé côté CPU ; pas de flou de mouvement sur le rig. |
| P5 | Évitement de parois par champ de distance, mondes tuilés/seamless, macro-variation, tailles jusqu'à 16 384², modes fast/refined/photoreal | Seamless C0 (pas de correction de pente), pas de Wang. |
| P6 | UV1 atlas de boîte, options Smart UV/remesh/save-blend, docs, démos, reprise | Atlas par sommet (coutures possibles) ; remesh délégué à Blender. |

- **Tests** : 152 verts, clippy `-D warnings` propre.
- **Historique** : dépôt réinitialisé en un commit propre (`923669e`) pour ne conserver que le code et la documentation VistaForge.
- **Nouveaux crates** : `vf-detail`, `vf-mesh`, `vf-export`.
- **Démos** : `docs/screenshots/detail_rock.png`,
  `docs/screenshots/blender_cycles.png`, `docs/screenshots/rig_helicopter.png`,
  `docs/screenshots/world_seamless.png`.
- **UI** : compile, mais n'a pas été lancée sur un écran pendant la session.
- **Reprise** : ce document, `docs/SPEC_REALISME.md` (contrat et faisabilité),
  `docs/MONDE.md`, `docs/RIG.md`, `docs/BLENDER.md`.

---

## 0bis. Session du 14 novembre — falaises, lumière, modificateurs

- **Falaises** : pass `cliffs` (activé par défaut) qui façonne le heightfield
  sur les pentes > 48° : ladder de 18 cellules, poids lissé, enveloppe locale,
  seulement vers le bas. Murs verticaux continus, ombres et matériau rocheux
  normaux. Le mesh séparé du plan initial (rideau par cellule ou par arête) a
  été essayé puis abandonné (aiguilles détachées). Le modificateur
  `recast_cliff` (Poisson/SOR inspiré de Hesiod/HighMap) donne des murs plus
  lisses mais coûteux ; il reste optionnel.
- **Lumière** : AO à deux échelles (large 12 % + fine 6 cellules), courbe
  HBAO `gain(3)`, AO appliquée à la couleur finale, PCF 5×5 radial, biais
  dépendant de N·L, ombres **4096** en High, rebond chaud du sol, ciel
  d'horizon sur les faces verticales, normales de détail atténuées sur les
  faces raides.
- **Modificateurs** : nouveau crate `vf-modifiers`, 21 opérateurs
  (Displace, Warp, Transform, Swirl, Terrace, Fold, Stratify, Aperture, Clamp,
  Blur, Sharpen, Crater, Recast cliff, Erosion, Thermal, Alluvium, Snow,
  Dunes, Talus, Combine, BiasGain). Pile live appliquée au rendu ; CLI
  `terrain modifier list/add/set/remove/move/clear/bake` ; espace
  **Modificateurs** dans l'UI ; 7 outils MCP (`modifiers.*`). Schéma projet
  **v5** (champ `modifiers`, défaut vide, migration v1→v5 par serde).
- **Hesiod / HighMap** : reprise d'idées (profils gamma de strates, dune
  asymétrique, fill_talus, recast_cliff, HBAO, PCF). Implémentations Rust
  indépendantes, aucune ligne GPL copiée (projet MIT/Apache).
- **Tests** : **110 tests** verts, clippy `-D warnings` propre.
- **Démos** : `demo-out/qual/canyon_hero3.png`,
  `demo-out/qual/patagonia_hero3.png`, `demo-out/modifiers/gallery.png`.

## 0. Session qualité (ajouts du 13 au soir)

- **Ciel** : gradient à bande d'horizon nette, **étoiles procédurales**, preset
  `dusk` (crépuscule) avec soleil rasant et brume réglée.
- **Canyon** : opérateur de creusement (méandre + tributaire, fond plat), preset
  de roche `canyon`, cannelures verticales (`gullies`) sur les faces.
- **Palettes** : `dusk`, canyon désaturé, palette du preset modern appliquée
  automatiquement au CLI, option `--palette`.
- **Scènes arides** : plus de mer ni de neige, végétation réduite.
- **Granit** : opérateur `spires` (tours coniques, massif central) et preset
  `patagonia` (granit gris, eau turquoise, neige haute). Les tours sont
  maintenant appliquées après la stabilisation (sinon l'érosion les arrondit) :
  5 tours de 620 m pour 210 m de rayon. Résultat encore insuffisant — elles
  lisent comme des pointes sombres, pas comme les tours de Torres del Paine.
  Prochaine étape : parois verticales par mesh local (une hauteur par (x,y) ne
  peut pas produire un surplomb), et éclairage de face.
- **Docs** : `--palette`, `dusk`, `canyon`, `patagonia` documentés.

Images de référence produites : `demo-out/qual/canyon_hero.png` (canyon de jour),
`demo-out/qual/dusk_final.png` (crépuscule étoilé), `demo-out/qual/carve2.png`
(plateau + canyon). Le rendu reste un rasteriseur CPU sans textures : l'écart
avec les photos de référence tient surtout au détail de proche et à l'éclairage
global, pas à la structure géologique.

## 1. Résumé

En une nuit, VistaForge est passé d'un générateur de terrains avec rendu à un
outil avec **quatre modules nouveaux** :

1. **Interface en barres de fonctions** (sans graphe nodal) : 9 familles, un
   panneau latéral par famille, des lignes `libellé + slider + valeur
   numérique` partout.
2. **Modèle géologique procédural** : strates pendées, joints, colonnes
   basaltiques, karst, détail de surface ; 7 presets de roche.
3. **Matériaux** : éboulis, neige en plaques, végétation, humidité, normales de
   détail, ombrage des joints.
4. **Module de vol FPV/hélicoptère** : trajectoires, gîte physique, inertie,
   secousses, séquences PNG avec flou de mouvement, encodage MP4 si ffmpeg.

Le tout est testé (110 tests), clippy propre, versionné dans Git.

---

## 2. Ce qui fonctionne (vérifié sur cette machine)

| Domaine | État | Commande |
|---|---|---|
| Workspace Rust | compile, **152 tests verts**, clippy `-D warnings` | `cargo test --workspace` |
| Terrain Classic / Modern | 7 presets, taille jusqu'à 4096 | `terrain generate` |
| Géologie | 9 presets (dont `canyon`, `patagonia`), schéma projet v5 | `--rock canyon` |
| Matériaux | éboulis / neige / végétation / détail | rendu, panneau Matières |
| Érosion | hydraulique + thermique, réglable dans l'UI | panneau Érosion |
| Vol | 6 presets, 4 trajectoires, gîte et secousses | `anim still` / `anim render` |
| Ciel | crépuscule + étoiles, bande d'horizon | `scene preset <p> dusk` |
| Géologie sur DEM | applique un preset au heightfield courant | `terrain rock` |
| Flou de mouvement | accumulation 1–32 sous-images, obturateur 0–360° | `--shutter 180` |
| Vidéo | MP4 via ffmpeg, sinon séquence PNG + commande affichée | `--video out.mp4` |
| UI | barre de fonctions, panneaux, aperçu débouncé | `cargo run --release -p vf-app` |
| MCP | 29 outils dont `geology.set`, `modifiers.*`, `flight.info` | `vf-mcp --workspace .` |
| Git | dépôt initialisé, baseline + 8 commits | `git log --oneline` |

Mesures (64 cœurs, 750 Go) :

- rendu 960×540 : **477 ms** ; 1920×1080 SSAA2 : **5,6 s** ;
- 1026² généré + érodé + géologie : **~4 s** ;
- 2050² : génération ~15 s, rendu 1400×900 SSAA2 ~21 s ;
- séquence vol 720×405, 12 fps, flou 180° ×4 : **~1,3 s/image** (48 images en
  64 s).

---

## 3. Ce qui a été ajouté cette nuit

### 3.1 Interface (`crates/vf-ui/src/lib.rs`)

- `Workspace` : Terrain, Roche, Érosion, Eau, Matières, Lumière, Caméra, Vol,
  Rendu. Barre horizontale + panneau contextuel à gauche.
- Helpers `param_f32` / `param_u32` : libellé + slider + champ numérique +
  unité, double-clic pour revenir au défaut.
- Panneaux complets pour l'érosion, l'eau, les matières, la lumière, le vol.
- Scène par défaut : alpine moderne 514² avec érosion et grès.
- Aperçu de vol à un temps donné (slider), rendu en arrière-plan.

### 3.2 Géologie (`crates/vf-core/src/geology.rs`, `crates/vf-geology`)

- `GeologySettings` : bancs (épaisseur, pendage, azimut, dureté, relief,
  déformation, couleur), joints (maille, profondeur, largeur, gigue, réseau
  orthogonal / hex / radial), dissolution (densité, rayon, profondeur,
  cannelures), détail de surface.
- Opérateurs **conscients de la résolution** : une fracture plus étroite que
  2 cellules est élargie ; une doline plus petite que 2,5 cellules est agrandie.
- `strata_sample` partagé entre la géométrie et le shader : les bandes de
  couleur suivent les corniches.
- `joint_mask` pour les veines sombres du renderer.
- `micro_detail` appliqué **après** la passe de stabilisation : l'érosion ne
  peut plus effacer les aspérités.
- `build()` : pipeline unique `forme → érosion → géologie → stabilisation →
  détail`, utilisé par le CLI, l'UI et le MCP.
- 7 presets : granite, basalt, limestone, sandstone, schist, chalk, volcanic.

### 3.3 Matériaux (`crates/vf-core/src/scene.rs`, `crates/vf-render/src/shade.rs`)

- `Materials` : roche, détail (normale + échelle), éboulis (quantité + pente),
  neige (couverture + pente), végétation, humidité.
- Masques procéduraux : strates sur les faces, éboulis sur les pentes
  intermédiaires, neige en plaques (altitude + pente + bruit + orientation),
  herbe dans les replats, humidité près de l'eau.
- Normales de détail fractales, atténuées avec la distance (anti-aliasing).
- Éclairage ambiant : ciel entier (zénith → horizon), AO qui atténue sans
  éteindre, nuages atténués en incidence rasante.

### 3.4 Vol (`crates/vf-core/src/flight.rs`, `crates/vf-flight`)

- `FlightSettings` : trajectoire, durée, fps, vitesse, altitude sol, visée,
  gîte max, réactivité, uptilt, secousses, turbulence, nacelle, palette.
- Trajectoires générées : orbite, circuit, crête, piqué.
- Gîte = `atan(v²·κ/g)`, bornée et lissée ; courbure calculée sur fenêtre large
  pour rejeter le facettage de la polyligne.
- Attitude à retard du premier ordre, secousses fractales seedées, rafales.
- Visée du centre pour les orbites cinématiques (plan large).
- Garde au sol : hauteur max du terrain dans un rayon de 40 m + 4 m.
- CLI : `anim still`, `anim render` (séquence, `--stride`, `--shutter`,
  `--blur-samples`, `--video`).
- Flou de mouvement par accumulation de sous-images (vrai flou, pas un filtre).
- Presets : cinelifter, freestyle, racing, helicopter, cinematic, dive.

### 3.5 MCP (`crates/vf-mcp`)

- `geology.set` : preset, activation, palette assortie, régénération.
- `flight.info` : nombre d'images, gîte max, garde au sol minimale.
- `flight.still` : rendu d'une image du vol.
- 29 outils au total, dont 7 pour les modificateurs.

### 3.6 Démos (`demo-out/`)

- `geology/comparaison.png` : même caméra, sans/avec géologie.
- `geology/galerie_roches.png` : granite, basalte, calcaire, craie.
- `geology/sand2050.png` : grès 2050².
- `flight/cinematic.gif` : 44 images, 600×338, boucle.
- `flight/filmstrip.png` : planche de 6 images.
- `flight/final/` : 48 PNG 720×405.

---

## 4. Architecture

```
crates/
  vf-core      données : Seed, HeightField, Project (schéma v5), caméra, soleil,
               palette, matériaux, géologie, vol, érosion
  vf-terrain   resampling, brosses, normales, courbure, AO, hillshade
  vf-procgen   bruit, Classic, Modern, érosion
  vf-geology   strates, joints, karst, micro-détail, pipeline build()
  vf-modifiers 21 opérateurs de terrain, pile live appliquée au rendu
  vf-flight    trajectoires et dynamique de vol
  vf-scene     cadrage caméra, orbite, auto-frame
  vf-io        DEM, .vfproj, PNG, ASCII Grid
  vf-render    rasteriseur CPU, ombres, ciel, eau, matériaux, post
  vf-cli       binaire vistaforge (terrain, dem, scene, render, anim, preset)
  vf-mcp       serveur MCP stdio (29 outils)
  vf-ui        interface egui (barres de fonctions + panneaux)
  vf-app       binaire vistaforge-app
```

Règles respectées : `vf-core` sans dépendance lourde ; le rendu ne dépend pas
de l'UI ; CLI, UI et MCP partagent le même noyau. `vf-geology` dépend de
`vf-procgen` (bruit + érosion) ; `vf-flight` dépend de `vf-procgen` (bruit).

Schéma projet : **v5**. La migration accepte v1→v5 (les champs manquants
prennent leur valeur par défaut, dont la pile `modifiers` vide).

Pipeline terrain :

```
source fractale
  → érosion complète (si activée)
  → géologie (strates, joints, karst)
  → stabilisation (érosion légère : gouttes/4, talus ≤ 3)
  → micro-détail (aspérités finales)
```

---

## 5. Ce qui reste à développer

Par ordre de priorité recommandé.

### 5.1 Qualité visuelle (le sujet principal)

L'écart avec l'image de référence (montagne photoréaliste) reste important.
Diagnostic : ce n'est plus la géologie, c'est la **densité de détail** et les
**matériaux de proche**.

1. **Résolution de travail** : rendre les scènes de démo en 2050² (5 m/cellule)
   et prévoir un mode « qualité finale » dans l'UI. Coût : ~20 s par image.
2. **Détail de surface triplanar** : le détail actuel est une perturbation de
   normale en coordonnées monde ; il faut un repère tangent par face pour les
   falaises, sinon les aspérités « glissent ».
3. **Éboulis géométriques** : les cônes de débris au pied des falaises sont
   aujourd'hui une couleur, pas une forme. Opérateur : dépôt avec angle de
   repos + tri granulométrique.
4. **Neige** : les plaques sont là mais trop uniformes ; ajouter des congères
   (accumulation sous le vent) et des corniches.
5. **Végétation instanciée** : arbres/rochers en billboards ou maillages
   simples, masqués par pente/altitude/lithologie.
6. **Ciel et eau** : nuages encore plats à l'horizon, eau sans réflexion
   détaillée. Prévoir un cubemap de ciel ou un rendu de nuages volumétrique
   basse résolution.

### 5.2 Vol

- **Waypoints libres** : charger une trajectoire JSON (points + vitesses) en
  plus des motifs générés.
- **Vent et rafales directionnelles** : aujourd'hui isotropes.
- **Évitement de collision** : la garde au sol est un maximum local ; un vrai
  lissage de trajectoire éviterait les montées brutales.
- **Aperçu temps réel** : le viewport ne montre qu'une image fixe ; il faut un
  rendu basse résolution pendant la lecture (et cache shadow/AO).

### 5.3 Performance

- **Cache shadow map + AO** : pour une séquence, le soleil et le terrain sont
  fixes. `SceneData` contient des champs caméra et des champs statiques ; les
  séparer permettrait de réutiliser l'ombre et l'AO pour toutes les frames
  (gain estimé 1,5–2×).
- **Rendu tuilé adaptatif** : réduire l'échantillonnage des zones lointaines.

### 5.4 Robustesse

- Tests de migration de schéma (v1 → v5) avec un manifeste RON fabriqué.
- Test de non-régression visuelle (hash de heightfield pour chaque preset).
- `cargo deny` pour les licences.
- Validation de l'UI sur une machine avec écran (le serveur n'a pas de display ;
  l'UI compile mais n'a pas été vue tourner cette nuit).

### 5.5 Divers

- Export glTF / bridge Blender (prévu au plan).
- Mode Legacy 8 bits.
- Undo/redo.
- Surplombs/arches : mesh local plaqué sur les crêtes, plus tard.

---

## 6. Commandes de reprise

```bash
# Vérifications
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Interface
cargo run --release -p vf-app

# Scène géologique (grès)
cargo run --release -p vf-cli -- terrain generate \
  --mode modern --size 1026 --seed 42 --preset alpine \
  --rock sandstone --erode --droplets 60000 \
  --name "Grès" --out demo-out/geology/sand.vfproj

# Appliquer une roche à un DEM déjà importé
cargo run --release -p vf-cli -- terrain rock scene.vfproj --preset limestone

# Rendu gros plan
cargo run --release -p vf-cli -- render demo-out/geology/sand.vfproj \
  demo-out/geology/sand.png --quality high --width 1200 --height 800 \
  --camera 8100,5050,1180 --target 7395,5639,950 --fov 42 --sun 310,32 --exposure 1.3

# Vol : une image
cargo run --release -p vf-cli -- anim still demo-out/geology/sand.vfproj \
  demo-out/flight/still.png --preset cinematic --time 2 \
  --width 1200 --height 800

# Vol : séquence + flou de mouvement + MP4
cargo run --release -p vf-cli -- anim render demo-out/geology/sand.vfproj \
  demo-out/flight/final --preset cinematic --seconds 4 --fps 12 \
  --width 720 --height 405 --shutter 180 --blur-samples 4 \
  --video demo-out/flight/final.mp4

# Modificateurs : pile, réglage, cuisson
cargo run --release -p vf-cli -- terrain modifier add demo-out/qual/carve2.vfproj \
  terrace --param step_m=120 --param variation=0.4
cargo run --release -p vf-cli -- terrain modifier list demo-out/qual/carve2.vfproj --json
cargo run --release -p vf-cli -- terrain modifier bake demo-out/qual/carve2.vfproj

# Rendu avec ou sans falaises
cargo run --release -p vf-cli -- render demo-out/qual/carve2.vfproj \
  demo-out/qual/canyon.png --quality high --width 1400 --height 800 \
  --camera 3000,3000,1100 --target 6500,5200,400 --fov 60 --exposure 1.15
cargo run --release -p vf-cli -- render demo-out/qual/carve2.vfproj \
  demo-out/qual/canyon-no-cliff.png --preview --no-cliffs

# MCP
cargo run --release -p vf-mcp -- --workspace .
```

Note : `ffmpeg` n'est pas installé sur cette machine. La commande affiche la
ligne à lancer pour produire le MP4.

---

## 7. Décisions et pièges

1. **Géologie après l'érosion.** Une érosion complète après les strates efface
   toutes les corniches (le talus thermique à 38° aplatit les marches). D'où
   l'ordre : érosion → géologie → stabilisation légère → détail.
2. **Détail après la stabilisation.** Même raison : le détail doit être la
   dernière opération géométrique.
3. **Opérateurs conscients de la résolution.** Sans élargissement, les joints
   deviennent du bruit à 1026² et du crénelage à 2050².
4. **Détail vertical limité sur les faces raides.** Un déplacement vertical sur
   une paroi quasi verticale produit des stries, pas des aspérités. Le poids
   est réduit au-delà de 55 % de pente ; la rugosité des falaises doit venir
   des normales (triplanar à faire).
5. **Palette assortie désactivée par défaut.** `auto_palette` existe mais est
   off : appliquer « canyon » à un grès sur une scène alpine surprend.
6. **Nuages en incidence rasante.** Ils formaient une bande striée à l'horizon ;
   atténuation par angle et distance.
7. **Pas de cache d'ombre** pour l'instant : chaque frame reconstruit la shadow
   map. C'est le prochain gain de performance.
8. **UI non testée visuellement** : le serveur n'a ni écran ni GPU. Elle
   compile, les calculs sont testés, mais il faut la lancer sur un poste avec
   affichage pour valider la mise en page.

---

## 8. Tests

- 152 tests passent (`cargo test --workspace`), dont :
  - géologie : déterminisme, presets, invariance sur terrain plat ;
  - vol : déterminisme, garde au sol, gîte visible, cohérence `camera_at` ;
  - CLI : `anim still` de bout en bout ;
  - MCP : `geology.set` + `flight.info` dans une session JSON-RPC ;
  - projet : round-trip, masque + preview.
- clippy `-D warnings` propre sur tout le workspace.

---

## 9. Journal de la nuit

- **00:40** — Audit, veille concurrentielle, recherche roches, plan
  (`docs/PLAN_ADDITIONS.md`).
- **00:50** — `git init`, baseline. Première crate `vf-geology` (strates,
  joints, karst), CLI `--rock`, coloration des bancs, panneau Roche.
- **01:05** — UI refondue en barres de fonctions + panneaux contextuels.
- **01:20** — Matériaux v1, détail de surface, éclairage ambiant.
- **01:30** — `vf-flight` : trajectoires, dynamique, secousses. CLI `anim`.
- **01:45** — Flou de mouvement par accumulation, encodage MP4, garde au sol.
- **02:00** — Pipeline unifié `vf_geology::build`, palette assortie, docs
  (USAGE, README), MCP `geology.set` / `flight.*`, tests.
- **02:20** — Démos finales (comparaison, galerie, GIF de vol), rédaction de ce
  fichier.
- **18:45** — Reprise qualité : ciel à bande d'horizon, étoiles procédurales,
  preset `dusk`, opérateur canyon (méandre + tributaire), cannelures
  verticales, palettes désaturées, scènes arides sans mer ni neige, végétation
  supprimée sur les palettes arides.
- **19:35** — Rendus 2050² canyon et crépuscule, docs, tests, commit.
- **19:55** — Opérateur `spires` et preset `patagonia` (WIP), palette granite
  turquoise, ligne de neige relevée pour la Patagonie.
- **20:05** — Lumière/eau : reflet solaire sur l'eau réduit (4,0 → 1,3), plan
  de sol lointain ramené de 30 à 8 largeurs de terrain pour éviter la bande
  laiteuse à l'horizon.
- **20:10** — Normales de détail en repère tangent (triplanar) : le détail
  suit les faces au lieu de glisser en XY. Incident corrigé : l'édition avait
  supprimé `ground`, `skirt` et `water` ; ils ont été restaurés à l'identique
  (reflet 1,3 inclus) et le workspace compile.
- **20:20** — Détail renforcé (0,45 à 18 m) et veines de joints réservées aux
  faces raides : le treillis de Voronoi ne se voit plus sur les pentes douces.
  Il reste un grain fin, pas encore une texture de roche.
- **20:30** — Grain triplanaire dans l'albédo de la roche (échelle métrique,
  patches décamétriques). Effet invisible à 700 m (sous la taille du pixel),
  utile en vue rapprochée seulement. Confirme que le problème à distance est
  la silhouette et la lumière, pas la couleur.

---
### Journal du 14 novembre

- **21:00** — Essais de mesh de falaise : rideau par cellule, puis par arêtes de
  frontière. Résultat en aiguilles détachées, abandonné.
- **22:30** — Pass `cliffs` par ladder de hauteurs : masque de relief local,
  seuil 48°, poids lissé, enveloppe locale, seulement vers le bas.
- **23:30** — AO fine + AO large, courbe HBAO, PCF 5×5, ombres 4096, rebond.
- **00:15** — Crate `vf-modifiers`, 21 opérateurs, schéma v5, CLI/UI/MCP.
- **00:45** — Lecture de Hesiod/HighMap : profils gamma, dune asymétrique,
  `fill_talus`, `recast_cliff` ; implémentations Rust indépendantes.
- **01:00** — Démos canyon/Patagonie, galerie de modificateurs, docs, tests.


## 9bis. Analyse d'écart qualité

Voir [`docs/QUALITE_ECARTS.md`](QUALITE_ECARTS.md) : décomposition des trois
références, diagnostic mis à jour (la silhouette verticale est acquise, restent
la lumière, le matériau de paroi et l'avant-plan), plan technique, essais
négatifs et apports de Hesiod/HighMap. Les modificateurs sont documentés dans
[`docs/MODIFICATEURS.md`](MODIFICATEURS.md).

## 10. Prochaine session — actions prioritaires

1. **Lancer l'UI sur un poste avec écran** et valider les nouveaux espaces
   **Détail**, **Sculpter** et **Animation** (mise en page, drag des clés,
   pinceaux, aperçu). Le serveur n'a pas de display : tout compile, mais
   l'ergonomie n'a pas été vue tourner.
2. **Surplombs maillés** : la niche `undercut` est une approximation
   heightfield ; la lèvre qui dépasse (corniche, arche, champignon) demande un
   mesh local. C'est le prochain gain de réalisme sur les références envoyées
   (arche, cheminée de fée, orgue basaltique).
3. **DOF CPU** : le rig calcule distance de mise au point et ouverture ;
   Cycles les applique, le rasteriseur non. Ajouter un z-buffer de profondeur
   et un bokeh scatter-as-gather pour le mode photoreal.
4. **Seamless C1 / Wang** : corriger aussi les pentes aux bords et générer un
   jeu de tuiles de Wang pour un tuilage désordonné sans répétition.
5. **MCP** : exposer `detail.*`, `rig.*`, `world.*`, `brush.*` pour que
   l'agent puisse piloter les nouveaux modules.
6. **Cache ombre/AO** pour les séquences de rig, et rendu basse résolution
   temps réel dans l'UI.

Ensuite : végétation instanciée, nuages volumétriques, path tracing,
packaging.
