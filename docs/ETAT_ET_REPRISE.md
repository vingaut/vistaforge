# État du projet et reprise

> Document de reprise des sessions des **13, 14 et 15 novembre 2026**.
> Objectif : reprendre le travail sans relire l'historique, sur cette machine ou
> une autre. Il décrit ce qui fonctionne, ce qui a été ajouté, ce qui reste à
> faire et les pièges déjà rencontrés.

Dernière mise à jour : **2026-11-15 17:52** (lot terminé, binaires Linux publiés, dépôt poussé).

---

## 0quindecies. Session du 15 novembre — sculpture fine, animation, fichiers et Blender (livré)

> **État : lot terminé et poussé.** Le WIP « terrasses basaltiques » ci-dessous
> a été repris, complété et intégré. Workspace compilé, **223 tests verts**,
> `clippy -D warnings` propre, `cargo fmt` appliqué, captures UI et validation
> Blender 5.3 effectuées.
>
> **Git** : `origin/master` = `bb3ab21` sur
> <https://github.com/vingaut/vistaforge>. Commits du lot : `21aad3c`
> (code/docs), `859104a` (doc de reprise), `bb3ab21` (binaires). Les exécutables
> Linux x86_64 `vistaforge`, `vistaforge-app` et `vistaforge-mcp` sont dans
> `dist/linux-x86_64/` avec `SHA256SUMS`.

### Demande initiale

1. Reprendre l'amélioration de la **finesse des détails du mode Sculpter**
   (terrasses basaltiques, risers, orgues, aperçu).
2. Lire l'animation **en restant en vue design**, pas seulement en vue caméra.
3. Corriger la **durée d'animation** bloquée au-dessus de ~49,99 s.
4. Corriger les **profils de mouvement** (roulis frénétique, évitement de
   parois, cible par avance).
5. Ajouter une **timeline avec graph editor**, liée aux réglages de profil et
   d'animation, avec keyframing.
6. Déplacer les clés **librement à la souris**, en plus du gizmo et du
   numérique.
7. Fiabiliser lecture/sauvegarde et ajouter heightmaps PNG/TIFF/JPG,
   import/export DEM, bridge Blender (GLB) et export/import d'une animation
   caméra/cible **sans géométrie**.

### Livré

#### Sculpter

- `Brush::riser_sharpness` (0 = pente large, 1 = marche quasi verticale), exposé
  dans l'UI et le CLI (`--riser-sharpness`).
- `hillshade_sculpt` : hillshade supersamplé + **courbes de niveau** (majeures
  toutes les 5 lignes), avec réglage de l'intervalle.
- Aperçu 2× possible sur un terrain 2050² (cap ~24 M pixels au lieu de 4 M) ;
  l'ancien cap désactivait silencieusement le supersampling sur les gros DEM.
- Maillage GPU porté à **2 049 segments** : un terrain 2050² n'est plus
  sous-échantillonné (3× avant) ; 4096² reste à 2×.
- Densification ×2, avertissement cellule > 8 m, presets Terrasses basaltiques
  et Orgues de pente, tests bancs/risers/orgues/déterminisme.
- Doc dédiée : [`SCULPT.md`](SCULPT.md).

#### Rig et animation

- **Durée indépendante des clés** : suppression du bump automatique
  `duration >= eye.duration_s()` ; slider logarithmique + champ fin ; les clés
  au-delà de la fin sont conservées mais non jouées, avec bouton d'extension.
- **`RigTrack`** : simulation précalculée à 60 Hz, échantillonnage O(1) ;
  la lecture ne re-simule plus depuis t=0 à chaque frame.
- **Lecture en vue design** : le frustum, l'œil et la cible s'animent sans
  quitter l'orbite ; la vue caméra reste disponible. `Lire` lit la vue courante.
- **Cible/avance** : distance d'avance minimale de 45 m, lissage de visée
  effectif (filtre du premier ordre : `smoothing`, `look_lag`, 60 ms).
- **Évitement de parois** : champ de distance lissé (box blur + stencil 2
  cellules), sommet du mur le plus proche stocké, accélération d'évitement
  filtrée et bornée, désactivation au-dessus d'une falaise, roulis limité à
  120°/s avec objectif filtré.
- **Timeline + graph editor** : pistes Œil/Cible, retiming par glisser,
  insertion double-clic, suppression clic droit, zoom molette, canaux X/Y/Z,
  vitesse et roulis simulés par le profil actif ; graphe éditable en temps et
  valeur.
- **Glisser libre des clés** : plan de l'écran, `Maj` = vertical, `Alt` = poser
  sur le terrain, aimant ; gizmo et champs numériques conservés.
- **Export/import animation seule** : `.vfcam` (rig complet), `.glb` (caméra +
  cible, extras VistaForge), `.csv` (une ligne/image) ; réimport sans perte,
  reconstruction depuis translation/rotation d'un GLB Blender.
- Addon Blender `bridge/blender/vistaforge_io.py` : import GLB animation
  (sélection caméra, remplacement de la caméra statique, fps et plage
  d'images), export `.vfcam` depuis la caméra active + empty cible.

#### Fichiers et DEM

- **GeoTIFF 32 bits float** en lecture/écriture : `ModelPixelScale`,
  `ModelTiepoint`, GeoKeyDirectory (projeté, mètres, pixel-is-point),
  `GDAL_NODATA`. Lecture entiers/floats, multi-canal, masque nodata.
- **ESRI ASCII Grid**, **DEM legacy v1**, **PNG 16 bits + sidecar JSON + PGW**,
  **JPEG** (lossy), **raw f32 + JSON**.
- Import **PNG/TIFF/JPEG** avec plage min/max ou sidecar ; `0/0 = auto` ;
  retournement nord→sud ; dimensions jusqu'à 16 384².
- Espace **Fichiers** : projet, import/export terrain, export Blender
  (`scene.glb` + textures + `camera_animation.glb`), animation, navigateur de
  fichiers intégré (pas de dépendance native), `Ctrl+O`/`Ctrl+S`.
- Projet : sauvegarde atomique (le fichier précédent survit à un échec),
  validation du rig au chargement, réparation migration durée/fps/FOV,
  normalisation de l'extension `.vfproj`.
- Doc dédiée : [`FICHIERS.md`](FICHIERS.md).

### Correctif « la caméra ne suit pas la courbe »

Quatre causes distinctes ont été identifiées et corrigées :

1. **La durée ne rééchelonnait pas les clés.** La simulation échantillonnait la
   courbe à `t` avec les temps absolus des clés. Passer de 50 à 70 s laissait
   donc le tracé se terminer à 50 s : la caméra arrivait à la fin puis
   attendait. `CameraRig::stretch_duration` rééchelonne désormais les temps des
   clés (positions 3D inchangées) pour remplir le clip ; la case **Étirer les
   clés avec la durée** permet de revenir aux temps absolus.
2. **La caméra démarrait à l'arrêt.** `RigSimulation` initialisait `eye_vel` à
   zéro, d'où un retard inévitable de `v²/(2a)` au premier virage. Elle démarre
   maintenant à la vitesse du tracé, plafonnée à la vitesse max du profil.
3. **La traînée créait une erreur permanente.** La traînée était appliquée à la
   vitesse absolue sans poussée compensatoire : le ressort devait conserver un
   écart de plusieurs dizaines de mètres pour la vaincre. Une avance de poussée
   `path_velocity * drag` compense la traînée.
4. **L'ease par clé faisait sursauter la vitesse.** `PathKey::new` utilisait
   `ease = 0.5` (vitesse nulle à chaque clé, pic à 1,5×). Les nouvelles clés
   utilisent `ease = 0` (vitesse constante) ; le bouton **Vitesse constante
   (ease 0)** remet un tracé existant à zéro.

La durée conseillée inclut **15 % de marge** pour l'accélération, les virages et
la stabilisation finale. Un **bandeau de diagnostic** mesure l'écart réel entre
la trajectoire simulée et la courbe et propose : allonger la durée, ease 0, ou
profil Manuel. Test de non-régression : `recommended_duration_keeps_the_demo_flyover_on_its_curve` (drone, 4 clés, écart < 15 m).

### Validation

- `cargo test --workspace` : **223 tests verts** (dont GeoTIFF, heightmap
  PNG/JPEG, import/export animation, timeline, glisser libre, durée, parois).
- `cargo clippy --workspace --all-targets -- -D warnings` : propre.
- `cargo fmt --all` : appliqué.
- Captures eframe (glow) de tous les espaces, dont `workspace_files.png`,
  `animation_timeline.png`, `animation_graph.png`, `sculpt_preview.png`.
- **Blender 5.3** :
  - import du GLB caméra : `VistaForge Camera` + `VistaForge Target`, action
    `0..192`, positions caméra/cible distinctes à plusieurs frames ;
  - addon : import scène + animation, caméra active `VistaForge Camera`,
    0..192 à 24 fps, rendu Cycles 320×180 frame 96 ;
  - export `.vfcam` depuis Blender : 193 clés, réimport VistaForge
    `target Path`, 8 s ;
  - export `.vfcam` → `rig import` → `rig export` GLB testé.
- Import/export GeoTIFF 2050² : ~1 s, valeurs et origine conservées.
- PNG 16 bits + sidecar : round-trip 100..500 m, erreur de quantification
  inférieure à 0,1 m ; JPEG accepté mais lossy.
- `/tmp/vfcap-gpu` : projet 2050² chargé avec le viewport **wgpu/Vulkan**
  (Quadro RTX 5000), tour de captures complet, mesh 2049 segments ; pas de
  sous-échantillonnage 3× comme avant.

### Fichiers touchés (principaux)

| Fichier | Rôle |
|---|---|
| `crates/vf-terrain/src/brush.rs`, `lib.rs` | riser sharpness, hillshade sculpt + contours |
| `crates/vf-flight/src/rig.rs`, `clearance.rs` | RigTrack, cible lissée, évitement lissé, roulis |
| `crates/vf-ui/src/timeline.rs` | timeline + graph editor |
| `crates/vf-ui/src/animation_editor.rs` | glisser libre, frustum live |
| `crates/vf-ui/src/file_picker.rs` | navigateur de fichiers intégré |
| `crates/vf-ui/src/lib.rs` | espace Fichiers, transport design, timeline, IO UI |
| `crates/vf-ui/src/help.rs` | fiches mises à jour |
| `crates/vf-io/src/geotiff.rs`, `heightmap.rs`, `terrain_io.rs` | formats DEM/heightmap |
| `crates/vf-io/src/project.rs` | sauvegarde atomique, migration rig |
| `crates/vf-export/src/animation.rs` | `.vfcam`, GLB caméra, CSV |
| `bridge/blender/vistaforge_io.py` | addon Blender aller-retour |
| `docs/FICHIERS.md`, `docs/SCULPT.md`, `docs/ANIMATION.md`, `docs/BLENDER.md`, `docs/USAGE.md` | documentation |

### Limites restantes

- Les poignées Bézier restent éditables numériquement (ease/handles) mais pas
  par un graphe de poignées dédié ; la timeline édite temps et valeurs.
- Le GeoTIFF utilise un CRS **utilisateur** en mètres ; renseigner le CRS réel
  dans QGIS/GDAL pour une superposition géographique.
- Le round-trip **géométrie** Blender reste aller seulement ; le round-trip
  **trajectoire** est assuré (`.vfcam`/GLB + addon).
- JPEG et DEM v1 sont lossy.
- Le mesh GPU à 2 049 segments peut être lourd sur une petite carte ; le rendu
  final reste CPU.
- Pas encore de MP4 depuis l'UI (CLI seulement), ni de dialogue natif de
  fichiers (le navigateur intégré le remplace).

### Commandes de reprise

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p vf-app -- demo-out/geology/alpine.vfproj

# Terrain / DEM
cargo run -p vf-cli -- terrain import dem.tif --out dem.vfproj
cargo run -p vf-cli -- terrain export dem.vfproj dem.tif

# Animation seule
cargo run -p vf-cli -- rig export scene.vfproj survol.glb
cargo run -p vf-cli -- rig import scene.vfproj survol.vfcam --out rejoue.vfproj

# Blender
cargo run -p vf-cli -- blender export scene.vfproj blender-out --subdivision 2
blender --background --python bridge/blender/vistaforge_import.py --   blender-out/scene.glb blender-out/scene.json --validate
```

---

## 0quaterdecies. Session du 15 novembre — sculpt de terrasses basaltiques (WIP intégré)

> **État : section historique.** Le travail décrit ci-dessous a été repris et
> terminé dans la section précédente. Le lot est commité (`21aad3c`).

### Demande

L'utilisateur a sculpté des terrasses et des sillons dans l'espace **Sculpter**
(scène 514×514, cellules de 25 m). Résultat jugé médiocre dans la vue 3D, et
**définition insuffisante dans le module de sculptage** (hillshade très bruité,
facettes, motif en pochoir). Il a joint une photo de référence : une **falaise
basaltique stratifiée**, avec de nombreux bancs fins, des risers raides, des
orgues verticales et de la végétation sur les replats.

Objectif : rendre le pinceau Terrasse/Groove plus proche de cette morphologie
et améliorer la définition de l'aperçu de sculpture.

### Ce qui a été implémenté (WIP)

#### Modèle de pinceau (`crates/vf-terrain/src/brush.rs`)

- Nouveaux champs de `Brush` : `dip_deg`, `dip_azimuth_deg`, `bench_ratio`,
  `bench_variation`, `column_spacing_m`, `column_depth_m`, `column_jitter`,
  avec validation.
- **Terrasse stratifiée** : les couches sont quantifiées dans un repère
  incliné (pendage + azimut), en coordonnées **monde** pour que les bancs
  s'alignent entre deux traits qui se chevauchent. Chaque couche est un banc
  plat (`bench_ratio`) suivi d'un **riser étroit** (largeur fixe ~35 % de la
  transition, pas un long smoothstep). L'épaisseur varie avec un bruit basse
  fréquence (`bench_variation`) et une rugosité légère.
- **Orgues verticales** : option `column_spacing_m` / `column_depth_m` /
  `column_jitter`. Les rainures sont creusées uniquement sur les risers, avec
  un décalage irrégulier. Coordonnées globales pour la continuité entre traits.
- **Groove aligné sur la pente** : quand `column_spacing_m > 0`, les sillons
  sont orientés selon la **descente locale** (gradient), avec un espacement
  périodique, un jitter et une pondération par la pente. L'ancien mode bruit
  reste disponible si l'espacement vaut 0.
- **Correction d'un bug de gradient** : `gradient_world` passait des coordonnées
  déjà converties à un helper `sample_bilinear` qui reconvertissait en monde,
  divisant la pente par l'espacement. Les orgues étaient donc trop peu
  profondes. Corrigé (`hf.sample_bilinear` directement).
- Tests ajoutés : bancs plats + risers raides, canaux périodiques, déterminisme.

#### Aperçu de sculpture (`crates/vf-terrain/src/lib.rs`, `vf-ui`)

- Nouvelle fonction `hillshade_supersampled(hf, params, factor)` (facteur 1–4) :
  échantillonnage bilinéaire entre les centres de cellules, normales lissées avec
  un stencil d'une cellule, image agrandie `width×factor` par `height×factor`.
  But : supprimer le motif en pochoir du hillshade 1×/cellule.
- Champs UI `sculpt_detail` (1×/2×/3×, capé à ~4 M de pixels) et
  `sculpt_exaggeration` (0,5–4×, défaut 1,8).
- Nouvelle section **Définition** dans l'espace Sculpter : résolution, taille de
  cellule, relief, choix de l'aperçu, exagération, bouton **Densifier ×2**.
- **Densifier ×2** : rééchantillonnage bicubique jusqu'à 4096², mise à jour de
  `Project::sync_terrain`, vidage de l'undo/redo (les régions ne sont plus
  valides), régénération du hillshade et du mesh GPU, message de statut.
- Avertissement si cellule > 8 m : « les terrasses fines demandent ≤ 8 m/cellule ».

#### Interface Sculpter (`crates/vf-ui/src/lib.rs`)

- Section « Strates et colonnes » pour Terrasse : épaisseur des bancs,
  inclinaison, azimut du pendage, part banc/falaise, variation, espacement /
  profondeur / irrégularité des orgues.
- Section « Orgues de pente » pour Groove : espacement et irrégularité.
- Presets ajoutés : **Terrasses basaltiques** (bancs de 8 m, pendage 3°,
  colonnes 18 m, profondeur 2,5 m) et **Orgues de pente** (colonnes 12 m,
  profondeur 3,5 m).
- Au changement de mode, valeurs par défaut cohérentes (Terrasse = mélange 1,
  Groove = profondeur 4 m, etc.). Libellés d'intensité corrigés :
  « Mélange » pour Terrasse/Smooth/Flatten/Clone, « Profondeur » pour Groove.
- Aide contextuelle de l'espace Sculpter mise à jour (résolution, densification,
  bancs/risers, orgues, aperçu 2×–3×).

#### CLI (`crates/vf-cli/src/main.rs`)

- Nouvelles options `terrain sculpt` : `--dip`, `--dip-azimuth`,
  `--bench-ratio`, `--bench-variation`, `--column-spacing`, `--column-depth`,
  `--column-jitter`.
- **Manque encore `--hardness`** : la dureté n'est pas exposée par le CLI,
  seulement par l'UI. Les commandes de démo ci-dessous l'omettent.

### Vérifications faites

- `cargo test --workspace` : **191 tests verts** (3 ajoutés : hillshade
  supersamplé, terrasse, orgues).
- `cargo clippy --workspace --all-targets -- -D warnings` : propre après
  correction d'un `manual_clamp`.
- `cargo fmt -p vf-terrain -p vf-ui -p vf-cli` : appliqué.
- Tests brush : 9/9.
- Le code compile (`cargo check -p vf-app` via les tests).
- **Pas encore** : capture et inspection visuelle du nouveau panneau Sculpter,
  test du bouton Densifier dans l'UI, validation de la lisibilité du hillshade
  2×/3×, comparaison avant/après avec la référence.

### Essais visuels (fichiers temporaires, non versionnés)

Un projet de démonstration a été fabriqué à partir de `demo-out/qual/carve2.vfproj`
(2050², 4,9 m/cellule) :

```bash
cp demo-out/qual/carve2.vfproj /tmp/terrace2.vfproj
cargo run -p vf-cli -- terrain sculpt /tmp/terrace2.vfproj \
  --x 3800 --y 4200 --x2 6200 --y2 5200 \
  --mode terrace --radius 450 --strength 1 --step 6 --dip 4 \
  --bench-ratio 0.90 --bench-variation 0.45 \
  --column-spacing 12 --column-depth 3.5 --column-jitter 0.5 \
  --falloff smooth --roughness 0.08 --aa 4
cargo run -p vf-cli -- render /tmp/terrace2.vfproj /tmp/terrace2_close.png \
  --preview --width 1200 --height 800 --camera 5600,4300,1100 \
  --target 5000,5000,700 --fov 45 --sun 310,35 --exposure 1.15
```

Rendus : `/tmp/terrace1.png`, `/tmp/terrace2_close.png`,
`/tmp/terrace3_close.png` (ce dernier sur un versant raide autour de
`(3300, 3750)`).

**Constat** : la structure en bancs est là, les orgues verticales apparaissent
sur les parois, mais les risers restent **arrondis et larges**. À 4,9 m/cellule,
les bancs de 5–6 m sont à peine résolus et le rendu lisse les marches. Le
résultat est plus proche de la référence qu'avant, mais pas encore convaincant.
Le vrai gain passera par une résolution de 2–3 m/cellule et/ou une passe de
raffermissement local des risers.

Un rendu sur un versant très raide (moyenne de pente ~2,2) a été tenté mais la
caméra était collée à la paroi et l'image n'est pas exploitable (`terrace3_close.png`).
Il faut recadrer et probablement réduire le rayon pour éviter de tout lisser.

### État Git

- Travail **non commité**, **non poussé**.
- Dernier commit : `afa4c0c Merge flight into animation, add path editor and contextual help`.
- Fichiers modifiés (5) :
  - `crates/vf-terrain/src/brush.rs`
  - `crates/vf-terrain/src/lib.rs`
  - `crates/vf-ui/src/lib.rs`
  - `crates/vf-ui/src/help.rs`
  - `crates/vf-cli/src/main.rs`
- Aucun nouveau fichier de documentation ni capture n'a été ajouté pour ce lot.
- Les projets et rendus de démo sont dans `/tmp`, pas dans le dépôt.

### Prochaines étapes conseillées, dans l'ordre

1. **Valider l'UI** : lancer `cargo run -p vf-app -- demo-out/qual/carve2.vfproj`,
   ouvrir Sculpter, vérifier la section Définition, le hillshade 2×/3×, le
   bouton Densifier (undo vidé, mesh GPU mis à jour), les sliders strates/orgues.
2. **Tester sur une vraie paroi raide** : densifier à 2–3 m/cellule, rayon
   100–200 m, `bench_ratio` 0.9, colonnes 6–10 m, profondeur 2–3 m ; recadrer
   la caméra à 200–400 m de la paroi.
3. **Raffermir les risers** :
   - réduire encore la largeur de transition ;
   - ajouter un paramètre `riser_sharpness` ou une passe de sharpening local
     (cliff) après le stamp ;
   - envisager un mesh local pour les surplombs, impossible en heightfield.
4. **Ajouter talus / caprock / végétation de replat** : le pinceau peut
   adoucir le pied de falaise et préserver une dalle dure au sommet ; le
   matériau doit verdir les bancs plats.
5. **Réduire l'écart de définition** : vérifier le coût du hillshade 4096² et
   du densify ; éventuellement calculer le hillshade dans un worker.
6. **GPU** : le mesh était plafonné à 1024 segments ; à 2050² il sous-échantillonnait. Corrigé à 2 049 segments dans le lot suivant.
   Augmenter le plafond ou ajouter un LOD pour que la vue 3D montre les bancs.
7. **CLI/docs** : ajouter `--hardness`, documenter les nouvelles options,
   créer `docs/SCULPT.md`, refaire `docs/help/sculpt.png` avec un avant/après
   réel, mettre à jour USAGE et ce fichier une fois le lot terminé.
8. **Tests** : ajouter un test UI du densify (taille, undo vidé), un test de
   continuité des colonnes entre deux stamps, et un test de rendu/normal.
9. **Commit/push** quand le rendu est validé.

### Commandes de reprise

```bash
cargo test --workspace          # 191 tests attendus
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p vf-app -- demo-out/qual/carve2.vfproj

# Densifier un projet avant de sculpter
cargo run -p vf-cli -- terrain resample /tmp/terrace.vfproj --width 4096 --filter bicubic

# Terrasse + orgues (sans --hardness, absent du CLI)
cargo run -p vf-cli -- terrain sculpt /tmp/terrace.vfproj \
  --x 3100 --y 3550 --x2 3500 --y2 3900 \
  --mode terrace --radius 180 --strength 1 --step 4 --dip 5 \
  --bench-ratio 0.92 --bench-variation 0.5 \
  --column-spacing 8 --column-depth 2.5 --column-jitter 0.5 \
  --falloff smooth --roughness 0.05 --aa 4
```

---

## 0terdecies. Session du 15 novembre — finition Animation et aide (état livré)

> Cette section **remplace** l'état « en cours » du 14 novembre. Elle décrit ce
> qui est réellement livré, ce qui a été corrigé et ce qui reste ouvert.

### Demande

1. Fusionner **Vol** et **Animation** : un seul parcours, et un moyen évident de
   lancer l'animation une fois les réglages terminés.
2. Éditer le tracé de la caméra **et** le point de vue associé, matérialisé
   seulement dans le viewport de conception, sur les trois axes avec un gizmo.
3. Accéder ensuite aux réglages standards (durée, fps) puis au profil de vol,
   avec une différence de mouvement lisible entre drone, hélicoptère, etc.
4. Ajouter une **aide dynamique** dans le viewport, liée au contrôle survolé :
   captures annotées et effets attendus, pour toutes les rubriques.

### Livré

- **Un seul espace Animation**, quatre onglets : `1 · Tracé`, `2 · Réglages`,
  `3 · Profil`, `4 · Lecture`. L'onglet Vol a disparu ; `FlightSettings` reste
  uniquement dans le modèle et le CLI pour compatibilité.
- **Barre de transport ancrée dans le viewport**, visible dans tous les onglets :
  Début, Lire/Pause, curseur de temps, image −/+, Boucle, Design/Caméra,
  nombre d'images, durée, fps et profil. `Espace` = lire/pause.
- **Éditeur de tracé 3D** : tracé cyan, cible orange, clés E/T, viseur (tronc de
  frustum) et ligne de visée. Mode Tracer avec aperçu du point d'impact, de la
  verticale et de l'altitude. Gizmo X/Y/Z, aimant, insertion/suppression de clé,
  `Suppr`, `Échap`, cadrage du tracé. La sélection d'une clé cible bascule
  automatiquement l'onglet sur les clés de cible.
- **Profils de vol** : cartes, caractère, comparaison des courbes vitesse/roulis,
  superposition des trajectoires (corps actif en jaune), test en lecture,
  **tracé de démonstration (S)** adapté au profil et cadré automatiquement.
- **Calage durée/profil** : longueur du tracé, vitesse demandée vs croisière
  (75 % de la vitesse max), encadré d'avertissement et bouton **Ajuster la durée**.
- **Corrections du rig** (importantes pour la lisibilité des profils) :
  - `PathKey::new` utilise désormais des **poignées automatiques** (0,0,0) au
    lieu de ±300 m : les clés tracées ne bouclent plus.
  - Le ressort du rig reçoit une **avance de vitesse** `(path_velocity - eye_vel)`
    et les temps de réponse ne sont plus multipliés par 2π. Sans cela, les corps
    rapides saturaient l'accélération et oscillaient autour de la courbe.
  - Les valeurs de traînée des presets permettent à chaque corps d'atteindre sa
    vitesse max (le dirigeable était plafonné à ~3 m/s par la traînée).
  - Le tracé de démonstration est chronométré par longueur d'arc et vise 90 % de
    la vitesse max du corps actif.
- **Aide contextuelle** (`crates/vf-ui/src/help.rs`) :
  - **25 fiches** couvrant tous les espaces, les quatre onglets d'animation,
    l'optique, les contraintes, le rendu, le GPU, l'aperçu adaptatif et l'export.
  - Carte plus large et scrollable, résumé, puces ciblées, **capture annotée**,
    légende « effet attendu », « à essayer », valeurs live pour le tracé, la
    cible, le timing, l'optique, les contraintes, le profil et la lecture.
  - **Sommaire**, fiches précédente/suivante, épinglage, fermeture, `F1`,
    bouton `?`.
  - Enregistrement par **section entière** (union en-tête + corps) : survoler
    n'importe quel slider d'« Optique et caméra » ouvre la fiche Optique. Un
    fallback de panneau couvre les contrôles hors section (onglet courant).
  - La carte est dessinée **avant** le viewport : l'éditeur connaît son rectangle
    courant et ne confond plus un clic dans l'aide avec un clic terrain.
- **Mode capture** (`VISTAFORGE_CAPTURE`) : parcours intégré, attente de
  l'aperçu, capture eframe, écriture PNG et fermeture. Utilisé pour les captures
  de documentation et les visuels d'aide. Variables `VISTAFORGE_WORKSPACE`,
  `VISTAFORGE_ANIM_TAB`, `VISTAFORGE_HELP_TOPIC`.
- **Tests ajoutés** : ajout d'une clé par clic simulé en mode Tracer, hit-test du
  gizmo, priorité de résolution de l'aide, contenu/validité des 25 fiches,
  calage de durée, déterminisme des trajectoires comparées, différence de
  vitesse entre profils, poignées automatiques.
- **Docs** : `docs/ANIMATION.md`, `docs/AIDE.md`, README, USAGE, RIG, ce fichier.
- **Captures** : `docs/screenshots/gui.png` (Animation + aide),
  `animation_path.png`, `animation_profile.png` ; `docs/help/*.png` (21 visuels).

### Vérifications

- `cargo test --workspace` : **188 tests verts**.
- `cargo clippy --workspace --all-targets -- -D warnings` : propre.
- `cargo fmt -p vf-ui -p vf-flight -p vf-core -p vf-app` : appliqué.
- Application lancée sous X11/Xwayland avec `VISTAFORGE_RENDERER=glow` puis
  avec le renderer **wgpu/Vulkan** par défaut : tour de capture complet
  (Animation + 12 espaces), captures inspectées.
- Mode capture testé avec `demo-out/geology/alpine.vfproj` et `ile.vfproj`.

### Non validé / limites

- **Souris réelle** : XTest n'injecte ni mouvement ni clic dans cette session
  (Xwayland). Les interactions sont couvertes par des tests headless (clic de
  tracé, gizmo, priorité d'aide), mais le drag réel des axes, les boutons de la
  barre de transport et l'export n'ont pas été cliqués à la main.
- **GPU wgpu** : le tour de capture a aussi été exécuté avec le renderer wgpu
  par défaut (Quadro RTX 5000, Vulkan) ; les captures de `docs/screenshots/`
  viennent de ce passage. Les visuels `path/target/profile` de `docs/help/`
  sont également composés depuis ces captures GPU ; les schémas restent des
  composites déterministes.
- **Aide** : la carte n'est pas déplaçable ; elle peut masquer une partie du
  viewport. Les visuels sont embarqués (environ 9 Mo de PNG).
- **Rig** : poignées Bézier manuelles non éditables ; l'évitement de parois est
  un champ 2D qui ignore l'altitude (les comparaisons de profils le désactivent
  pour isoler la dynamique) ; le DOF n'est pas rasterisé côté CPU.
- **Export** : pas de MP4 depuis l'UI, pas de choix brouillon/final ; un export
  pleine résolution est long.
- **Ancien modèle** : `FlightSettings` et `anim --preset` restent dans le CLI,
  sans UI. Décider plus tard de le migrer ou de le retirer.

### Fichiers touchés

| Fichier | Rôle |
|---|---|
| `crates/vf-flight/src/rig.rs` | avance de vitesse, réponses, traînée, longueur/durée conseillée, démo S, centre du tracé |
| `crates/vf-core/src/rig.rs` | poignées automatiques de `PathKey::new` |
| `crates/vf-ui/src/lib.rs` | transport, onglets, profils, calage, aide, capture, cadrage démo |
| `crates/vf-ui/src/animation_editor.rs` | viseur, aperçu de tracé, comparaison, correction du clic, tests |
| `crates/vf-ui/src/animation_ui.rs` | `TimingFit`, trajectoires comparées, démo, tests |
| `crates/vf-ui/src/help.rs` | 25 fiches, carte, images, tests |
| `scripts/make_help_images.py` | visuels annotés depuis les rendus |
| `scripts/make_help_screenshots.py` | captures réelles annotées |
| `docs/ANIMATION.md`, `docs/AIDE.md` | documentation utilisateur |
| `docs/help/*.png`, `docs/screenshots/*` | visuels |

### Commandes de reprise

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
VISTAFORGE_RENDERER=glow cargo run -p vf-app -- demo-out/geology/alpine.vfproj

# Captures de documentation
VISTAFORGE_RENDERER=glow VISTAFORGE_CAPTURE=/tmp/vf-captures \
  cargo run -p vf-app -- demo-out/geology/alpine.vfproj
python3 scripts/make_help_images.py
VISTAFORGE_CAPTURES=/tmp/vf-captures python3 scripts/make_help_screenshots.py
```

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

## 0undecies. Session du 14 novembre — navigation CAO et viewport GPU

- **Vues CAO (`vf-scene`)** : `ViewPreset` (perspective, face, arrière, gauche,
  droite, dessus, dessous), `framed_view`, `fit_distance`. Le dessus utilise un
  `up` horizontal (`+Y`) et le dessous `-Y` pour éviter la singularité du
  look-at au zénith ; les tests vérifient l'absence de dégénérescence et le
  cadrage.
- **Gizmo d'orientation** (`vf-ui/src/view_gizmo.rs`) : cube solidaire de la
  caméra, faces cliquables, triade X/Y/Z et extrémités négatives, bouton
  **Défaut**, info-bulles et raccourcis `1`/`2`/`3`/`0`/`Home`. Tests d'interface
  headless : un clic simulé sur l'axe Z renvoie bien la vue Dessus, le bouton
  maison renvoie Home.
- **Aperçu adaptatif** : l'aperçu CPU démarre à 25–50 % de 960×540 pendant une
  interaction, puis se raffine (≈ 50 → 75 → 100 %) après ~200 ms. L'échelle
  interactive est réapprise du temps mesuré pour viser ~45 ms par passe. Le
  HUD affiche la résolution et le temps de la passe ; le panneau Rendu expose
  la case **Aperçu adaptatif** et les compteurs.
- **Viewport GPU wgpu/Vulkan** (`vf-ui/src/gpu_view.rs` + shaders WGSL) :
  maillage du heightfield (modificateurs + falaises), ciel + soleil, eau,
  brouillard ; rendu offscreen couleur + `Depth32Float`, puis blit dans egui
  (le pass egui n'a pas de depth). `eframe` utilise désormais wgpu par défaut,
  avec repli `VISTAFORGE_RENDERER=glow`.
- **Rendu final inchangé** : le bouton Render produit toujours l'image CPU
  déterministe ; le viewport GPU est un aperçu de navigation et n'intervient
  pas dans les exports. Voir `docs/GPU.md`.
- **UI** : première exécution réelle de l'interface sur écran (X11/Xwayland),
  captures GPU et CPU, vérification du repli OpenGL.
- **Tests** : 173 verts (dont 5 tests de maillage GPU, 3 tests de clic gizmo,
  5 tests d'échelle adaptative), clippy `-D warnings` propre.

---

## 0duodecies. Session du 14 novembre — fusion Vol/Animation, éditeur de tracé, aide contextuelle (archive)

> **État : implémentation fonctionnelle mais non terminée.** Le code compile,
> les tests passent et les écrans principaux ont été vérifiés au moins une
> fois. Les interactions souris réelles, la lecture, l'export et une partie de
> l'aide restent à valider. Ne pas considérer cette session comme livrée.

### Demande initiale

- Supprimer la séparation contre-intuitive entre **Vol** et **Animation**.
- Éditer un tracé de caméra **et** le point de vue associé, matérialisé
  uniquement dans le viewport de conception.
- Dessiner / éditer le tracé sur les trois axes avec un gizmo.
- Accéder ensuite aux réglages standards (durée, fps, etc.) puis au profil de
  vol, avec une différence de mouvement lisible entre drone, hélicoptère, etc.
- Ajouter une **aide dynamique** dans le viewport, liée à l'option survolée,
  avec des visuels annotés.

### Ce qui a été fait

- **Fusion des espaces** : `Workspace::Flight` a été supprimé et
  `Workspace::Rig` renommé `Workspace::Animation`. L'ancien panneau Vol et
  l'ancien canevas 2D du rig ont été retirés de l'UI. Le modèle `CameraRig` est
  désormais l'unique modèle d'animation de l'interface ; `FlightSettings` reste
  dans le projet et le CLI pour compatibilité, mais n'a plus d'onglet.
- **Nouvel espace Animation** (`crates/vf-ui/src/lib.rs`) avec quatre onglets :
  1. **Tracé** : liste des clés œil/cible, temps, XYZ, adoucissement,
     poignées automatiques, ajout/suppression, modes de cible, génération
     orbite/crête/piqué, mode **Tracer**.
  2. **Animation** : durée, fps, nombre d'images, seed, FOV, liaison caméra
     rigide/ressort, roulis, DOF, contraintes sol/parois.
  3. **Profil de vol** : cartes de presets (drone, oiseau, hélicoptère,
     dirigeable, chasse, manuel), description du caractère, courbes comparées
     vitesse/roulis, trajectoire simulée, réglages fins.
  4. **Aperçu & export** : timeline, lecture/pause, boucle, vitesse, vue
     design/caméra, rendu d'une image à t, export de séquence PNG avec
     progression et annulation.
- **Éditeur de tracé 3D** (`crates/vf-ui/src/animation_editor.rs`) :
  - tracé cyan de l'œil, tracé orange de la cible, clés E/T, marqueur du temps
    courant, ligne œil → cible ;
  - cible fixe ou animée matérialisée uniquement dans l'espace Animation ;
  - sélection d'une clé au clic, gizmo de translation X/Y/Z avec flèches et
    étiquettes, drag sur un axe, aimant (snap) en mètres ;
  - mode **Tracer** : clic sur le terrain (raycast + bisection) pour ajouter
    une clé, altitude réglable pour l'œil, ajout à la fin du tracé et
    allongement automatique de la durée ;
  - l'éditeur signale au viewport s'il consomme le pointeur, pour ne pas
    déclencher l'orbite en même temps.
- **Helpers d'animation** (`crates/vf-ui/src/animation_ui.rs`) : tabs,
  application d'un preset sans écraser le tracé, `ensure_animation` (première
  ouverture : active le rig et génère une orbite si le tracé est resté celui
  par défaut), courbes de comparaison des six profils, trajectoire simulée,
  projection monde → écran, raycast terrain, graphique vitesse/roulis.
- **Lecture** : `pump_animation` avance `rig_time`, échantillonne `CameraRig`,
  met à jour la caméra ; en GPU la vue est continue, en CPU un aperçu est
  relancé au plus toutes les 90 ms. Espace = lecture/pause. Entrer en vue
  caméra mémorise la caméra de design ; orbiter depuis la vue caméra repasse
  en vue design.
- **Export séquence PNG** : `AnimationExportTask` + thread, `RigSimulation`,
  `render_with_options`, fichiers `frame_00000.png`, progression, annulation
  par `AtomicBool`, dossier de sortie (`anim-out` par défaut).
- **Aide contextuelle** (`crates/vf-ui/src/help.rs`) :
  - carte flottante en bas à gauche du viewport, ouverte au survol d'un
    contrôle enregistré ;
  - titre, résumé, puces, visuel, bouton **Épingler**, fermeture, bascule `?`
    dans la barre d'outils ;
  - visuels : captures annotées `docs/help/path.png` (tracé + cible + gizmo) et
    `docs/help/gizmo.png` (cube), plus des schémas egui pour cible, durée,
    lecture, aperçu adaptatif, GPU et rendu CPU ;
  - thèmes enregistrés : onglets d'espaces, gizmo d'orientation, onglets
    Tracé/Animation/Profil/Aperçu, boutons Tracer/Sélection, cible, clés,
    cartes de profils, graphique, case GPU, case aperçu adaptatif, bouton
    Render.
- **Assets** : `docs/help/path.png`, `docs/help/gizmo.png` (captures annotées
  avec PIL à partir d'une capture de l'espace Animation).
- **Dépendance** : `image` ajoutée à `vf-ui` pour décoder ces captures au
  démarrage.

### Vérifications faites

- `cargo check -p vf-app` : OK.
- `cargo test --workspace` : **175 tests passent** (19 dans `vf-ui`, dont les
  tests de l'éditeur de tracé et le gizmo).
- `cargo clippy --workspace --all-targets -- -D warnings` : OK.
- `cargo fmt -p vf-ui -p vf-scene -p vf-app` : appliqué.
- Vérifié à l'écran (X11/Xwayland, GPU Vulkan Quadro RTX 5000) :
  - espace Animation en vue design, tracé + cible + clés + gizmo affiché sur
    une clé sélectionnée par code de test ;
  - onglet Profil : cartes, courbes comparées, légende ;
  - aide contextuelle ouverte au survol du bouton **Tracer**, capture annotée
    affichée ;
  - viewport GPU en vue design (la vue caméra et la lecture restent à tester).

### Ce qui n'a pas encore été validé

- **Interactions souris réelles** dans le gizmo de tracé : le rendu et le
  hit-test sont en place, mais XTest n'injecte pas les clics dans cette
  session ; seuls les tests headless du gizmo d'orientation couvrent le clic.
- **Vue caméra / lecture / pause / boucle** avec de vraies entrées.
- **Export PNG** : thread, progression, annulation et écriture des fichiers.
- **Échelle du graphique Profil** : le correctif vient d'exclure le profil
  Manuel de l'échelle (il suivait le tracé à vitesse libre et écrasait les
  autres courbes) ; il n'a pas été re-capturé après ce changement.
- **Couverture de l'aide** : les thèmes sont branchés sur les principaux
  contrôles, mais pas sur tous les sliders/panneaux ; les captures Profil et
  Aperçu manquent encore. La largeur de la carte est de 380 px (à agrandir).
- **CPU pur** : la lecture et l'export n'ont pas été testés avec
  `VISTAFORGE_RENDERER=glow`.
- **Formatage global** : `cargo fmt --all --check` signale toujours un drift
  préexistant dans 14 fichiers hors périmètre ; ne pas le confondre avec cette
  session.

### Problèmes connus / points à trancher

1. **Durée du tracé généré** : `preset_path` fournit une orbite de 8 s sur un
   terrain de ~15 km. La vitesse résultante est énorme et la dynamique des
   profils sature. Il faut soit calculer une durée conseillée à partir de la
   longueur du tracé et du profil, soit proposer un bouton « durée conseillée »,
   soit assumer un vol stylisé et l'indiquer dans l'aide.
2. **Profil Manuel** : il suit le tracé exact à vitesse libre. Il est exclu du
   graphique mais reste sélectionnable ; décider s'il faut le garder comme
   référence ou le retirer de la comparaison.
3. **Cache des profils** : `profile_curves` lance six simulations à l'ouverture
   de l'onglet Profil et à chaque changement ; c'est acceptable à 514² mais
   lent sur un grand terrain. La trajectoire simulée a son propre cache indexé
   sur `anim_revision` et n'est pas recalculée pendant un drag.
4. **Export** : utilise la résolution et la qualité du panneau Rendu; à
   pleine résolution c'est très long. Prévoir un choix « brouillon / final »,
   éventuellement une résolution réduite par défaut, et un test de
   l'annulation.
5. **Temps de lecture CPU** : `sample_rig` repart de 0 à chaque image ; en CPU
   l'aperçu est throttlé à 90 ms. En GPU la lecture est fluide. À documenter.
6. **Aide** : `resolve` utilise le `last_rect` de la frame précédente pour
   exclure les clics dans la carte ; en mode Tracer un clic très rapide sur la
   carte juste après son apparition pourrait ajouter une clé. À vérifier.
7. **Ancien modèle Vol** : toujours dans le projet et le CLI (`anim`), sans UI.
   Décider si on le conserve pour les scripts ou si on le migre vers `CameraRig`.
8. **`ensure_animation`** écrase un tracé par défaut si le rig était désactivé ;
   c'est voulu pour le premier lancement, mais un projet avec un rig désactivé
   et un tracé par défaut sera remplacé.

### Fichiers touchés / ajoutés cette session

| Fichier | Rôle |
|---|---|
| `crates/vf-ui/src/animation_editor.rs` | nouveau : overlay, clés, gizmo, tracé au clic |
| `crates/vf-ui/src/animation_ui.rs` | nouveau : tabs, presets, courbes, raycast, projection |
| `crates/vf-ui/src/help.rs` | nouveau : aide contextuelle, cartes, visuels |
| `crates/vf-ui/src/lib.rs` | fusion des espaces, panneaux Animation, lecture, export |
| `crates/vf-ui/Cargo.toml` | dépendance `image` |
| `docs/help/path.png`, `docs/help/gizmo.png` | captures annotées de l'aide |
| `docs/ETAT_ET_REPRISE.md` | ce document |

Non modifiés cette session (stables de la session précédente) :
`gpu_view.rs`, `view_gizmo.rs`, shaders WGSL, `vf-scene`, `vf-flight`.

### Reprise conseillée, dans l'ordre

1. Lancer l'app, ouvrir **Animation**, sélectionner une clé, tester le drag des
   trois axes, l'aimant, le clic sur une autre clé, le mode Tracer, Escape.
2. Tester **Vue caméra**, lecture/pause/espace, boucle, vitesse, retour design.
3. Ouvrir **Profil**, vérifier l'échelle après exclusion de Manuel, appliquer
   hélicoptère/dirigeable, regarder la trajectoire simulée jaune.
4. Tester **Export** sur une petite résolution (320×180, qualité Low) dans un
   dossier temporaire, vérifier progression et annulation.
5. Faire un passage **CPU** (`VISTAFORGE_RENDERER=glow`) : aide, traceur,
   lecture, export.
6. Compléter l'aide : captures Profil et Aperçu, élargir la carte, enregistrer
   les sliders importants (FOV, durée, fps, profils).
7. Trancher la durée par défaut et le sort du profil Manuel.
8. Mettre à jour `README.md`, `docs/USAGE.md`, `docs/GPU.md` et les captures
   pour refléter la fusion Vol/Animation et l'aide.
9. Relancer `cargo test --workspace`, clippy, fmt ciblé, et refaire une capture
   d'écran de l'onglet Animation et du Profil.

### Commandes utiles

```bash
cargo run -p vf-app
VISTAFORGE_RENDERER=glow cargo run -p vf-app
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt -p vf-ui -p vf-scene -p vf-app
```

---

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
| Workspace Rust | compile, **175 tests verts**, clippy `-D warnings` | `cargo test --workspace` |
| Terrain Classic / Modern | 7 presets, taille jusqu'à 4096 | `terrain generate` |
| Géologie | 9 presets (dont `canyon`, `patagonia`), schéma projet v5 | `--rock canyon` |
| Matériaux | éboulis / neige / végétation / détail | rendu, panneau Matières |
| Érosion | hydraulique + thermique, réglable dans l'UI | panneau Érosion |
| Vol | 6 presets, 4 trajectoires, gîte et secousses | `anim still` / `anim render` |
| Ciel | crépuscule + étoiles, bande d'horizon | `scene preset <p> dusk` |
| Géologie sur DEM | applique un preset au heightfield courant | `terrain rock` |
| Flou de mouvement | accumulation 1–32 sous-images, obturateur 0–360° | `--shutter 180` |
| Vidéo | MP4 via ffmpeg, sinon séquence PNG + commande affichée | `--video out.mp4` |
| UI | barre de fonctions, panneaux, gizmo CAO, viewport GPU wgpu/Vulkan, aperçu CPU adaptatif | `cargo run --release -p vf-app` |
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
- **Lecture de séquence** : le viewport GPU couvre l'orbite et le cadrage en
  temps réel ; la lecture d'une séquence de vol complète reste un rendu CPU
  (CLI `anim render`), avec le cache shadow/AO à ajouter.

### 5.3 Performance

- **Cache shadow map + AO** : pour une séquence, le soleil et le terrain sont
  fixes. `SceneData` contient des champs caméra et des champs statiques ; les
  séparer permettrait de réutiliser l'ombre et l'AO pour toutes les frames
  (gain estimé 1,5–2×).
- **Rendu tuilé adaptatif** : réduire l'échantillonnage des zones lointaines.
- **Aperçu** : fait (résolution dynamique + raffinement progressif + viewport GPU).
  Reste à porter ombres/AO/nuages dans le shader GPU et à déporter la
  reconstruction du maillage sur un worker pour les très grandes grilles.

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
7. **Pas de cache d'ombre** pour le rendu CPU : chaque frame reconstruit la
   shadow map. Le viewport GPU n'a pas encore d'ombres.
8. **UI validée visuellement** le 14 novembre sur X11/Xwayland, avec le
   viewport GPU Vulkan (Quadro RTX 5000) et le repli `VISTAFORGE_RENDERER=glow`.

---

## 8. Tests

- 175 tests passent (`cargo test --workspace`), dont :
  - géologie : déterminisme, presets, invariance sur terrain plat ;
  - vol : déterminisme, garde au sol, gîte visible, cohérence `camera_at` ;
  - CLI : `anim still` de bout en bout ;
  - MCP : `geology.set` + `flight.info` dans une session JSON-RPC ;
  - projet : round-trip, masque + preview ;
  - vues CAO : cadrage, pôle non dégénéré, aller-retour yaw/pitch ;
  - gizmo : clic simulé sur l'axe Z et sur le bouton Défaut ;
  - aperçu adaptatif : convergence et monotonicité du raffinement ;
  - maillage GPU : eau, plafond de résolution, normales ;
  - éditeur de tracé : édition des composantes œil/cible, distance aux segments ;
  - aide contextuelle : non testée unitairement à ce stade.
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
