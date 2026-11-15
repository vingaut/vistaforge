# Utiliser VistaForge

VistaForge transforme un terrain fractal ou un DEM réel en image de paysage.
Le même noyau alimente le CLI, l'interface de bureau et le serveur MCP.

## Prise en main

```bash
# Un projet 1026² de style classique, avec érosion, puis un rendu 1080p
cargo run --release -p vf-cli -- terrain generate \
  --mode classic --size 1026 --seed 42 --island --erode \
  --name "Mon île" --out ile.vfproj

cargo run --release -p vf-cli -- render ile.vfproj ile.png \
  --quality high --width 1920 --height 1080

# Interface graphique
cargo run --release -p vf-app
```

Sans `--seed`, un seed aléatoire est tiré puis affiché : conservez-le pour
reproduire exactement le paysage.

## Génération de terrain

| Mode | Paramètres principaux | Style |
|---|---|---|
| `classic` | `--dimension` (0..200), `--divisor` 1/2/4/8, `--island` | midpoint displacement, esprit classique |
| `modern` | `--preset`, `--ridge`, `--warp`, `--continent` | fBm + ridged multifractal, domain warp, cratères, terrasses |

Les presets modernes sont `alpine`, `desert`, `volcanic`, `coastal`, `canyon`,
`lunar`, `island`. Les paysages générés couvrent environ 10 km ; le relief est
réglé par `--vertical-scale` (mètres).

```bash
cargo run --release -p vf-cli -- terrain generate \
  --mode modern --size 514 --seed 7 --preset alpine \
  --name "Alpes" --out alpes.vfproj --erode --droplets 25000
```

L'érosion combine des gouttes hydrauliques et une relaxation thermique. Elle
est déterministe et proportionnelle au seed.

### Géologie

Un modèle de roche s'applique après l'érosion, puis une passe de
stabilisation légère, puis un détail de surface :

```bash
vistaforge terrain generate --mode modern --size 1026 --seed 42 \
  --preset alpine --rock sandstone --erode \
  --name "Grès" --out gres.vfproj
```

Presets : `granite`, `canyon`, `basalt`, `limestone`, `sandstone`, `schist`,
`chalk`, `volcanic`. Le preset `canyon` creuse un méandre profond et un
tributaire dans un plateau, avec cannelures verticales sur les parois.

Le preset modern applique sa palette assortie ; `--palette` force un autre
choix (`alpine`, `desert`, `volcanic`, `coastal`, `canyon`, `lunar`, `island`,
`dusk`). Chacun règle les bancs (épaisseur, pendage, dureté), les fractures
(maille, profondeur), la dissolution (dolines) et le détail. Les réglages sont
ensuite modifiables dans l'onglet **Roche** de l'interface.

Les opérateurs sont conscients de la résolution : une fracture plus étroite
que deux cellules est élargie, une doline plus petite que 2,5 cellules est
agrandie.

Pour appliquer une roche à un DEM déjà importé, sans régénérer le fractal :

```bash
vistaforge terrain rock alpes.vfproj --preset limestone --out alpes-calcaire.vfproj
```

## Sculpture manuelle

L'espace **Sculpter** de l'interface affiche le heightfield à plat, ombré en
relief, et permet de peindre directement dessus. Les distances sont en mètres,
donc un pinceau de 80 m garde sa taille si le terrain est rééchantillonné.

- **Modes** : élever, abaisser, lisser, aplatir, terrasse, crête, sillon,
  cratère, bruit fractal, warp, cloner.
- **Formes** : disque, carré, losange, anneau, étoile, polygone.
- **Réglages** : rayon, aspect, angle, intensité, dureté, rugosité, échelle du
  bruit, anti-aliasing (1–8×), retombée (constante/linéaire/lisse/gaussienne/
  plateau), symétrie X/Y/XY/radiale.
- **Undo/redo** : chaque trait stocke uniquement la région modifiée.

En CLI, un stamp ou un trait droit est reproductible :

```bash
cargo run --release -p vf-cli -- terrain sculpt ile.vfproj \
  --x 5000 --y 5000 --x2 5600 --y2 5200 \
  --mode raise --shape star --radius 240 --strength 12 \
  --roughness 0.35 --aa 6 --out ile_sculpte.vfproj
```

## Modificateurs de terrain

Le projet embarque une **pile de modificateurs** appliquée au rendu, dans
l'ordre, après le pipeline de base. Elle ne modifie pas le heightfield stocké
tant qu'on ne la cuit pas. La référence complète (27 opérateurs, paramètres,
MCP) est dans [`MODIFICATEURS.md`](MODIFICATEURS.md).

```bash
# Ajouter un modificateur et régler ses paramètres
vistaforge terrain modifier add scene.vfproj terrace \
  --param step_m=120 --param strength=0.9 --param variation=0.4

# Lister la pile, la modifier, la réordonner
vistaforge terrain modifier list scene.vfproj
vistaforge terrain modifier set scene.vfproj 0 --param amount_m=200
vistaforge terrain modifier set scene.vfproj 0 --disable
vistaforge terrain modifier move scene.vfproj 1 --up

# Appliquer la pile au terrain enregistré et la vider
vistaforge terrain modifier bake scene.vfproj
```

Opérateurs : `displace`, `warp`, `transform`, `swirl`, `terrace`, `fold`,
`stratify`, `aperture`, `clamp`, `blur`, `sharpen`, `crater`, `recast_cliff`,
`erosion`, `thermal`, `alluvium`, `snow`, `dunes`, `talus`, `combine`,
`bias_gain`. Chacun a des paramètres nommés, listés par
`terrain modifier list --json`.

Le rendu, les aperçus et les séquences de vol appliquent la pile
automatiquement. Pour les gros modificateurs (`recast_cliff`), préférez un
`bake` avant une séquence de vol longue.

## Rendu

```bash
# Rendu final
vistaforge render scene.vfproj out.png --quality high --width 1920 --height 1080

# Aperçu rapide
vistaforge render scene.vfproj preview.png --preview --width 960 --height 540

# Soleil et caméra ponctuels (azimut boussole : 0 = sud, 90 = ouest)
vistaforge render scene.vfproj sunset.png --sun 45,8 --exposure 1.2 --no-ao

# Caméra explicite (coordonnées monde en mètres)
vistaforge render scene.vfproj closeup.png \
  --camera 8000,-6000,3000 --target 5000,5000,600 --fov 40
```

Options utiles : `--no-shadows`, `--no-ao`, `--no-clouds`, `--no-water`,
`--cliffs` / `--no-cliffs`, `--ssaa 1|2|3`, `--quality low|medium|high|ultra`,
`--json`.

Qualités : Low 720p sans ombre, Medium 1080p avec ombres 2048, High 1080p +
AO + SSAA 2 + **ombres 4096**, Ultra 4K + SSAA 2 + ombres 4096. Les dimensions
passées par `--width/--height` priment sur le preset.

Le pass **falaises** (`cliffs`, activé par défaut) façonne les faces raides en
parois verticales avant la génération du maillage. Il s'applique au terrain
affiché, pas au fichier ; `--no-cliffs` rend le terrain brut.

## Animation

L'espace **Animation** réunit ce que l'ancienne version séparait en **Vol** et
**Animation**. Un seul parcours, en quatre étapes :

1. **Tracé** — dessinez le point de vue au clic sur le terrain (mode Tracer,
   altitude réglable), puis déplacez une clé au gizmo X/Y/Z, par **glisser
   libre** directement dans le viewport, ou par valeurs numériques. La cible est
   matérialisée dans le viewport de conception uniquement. Le dock **Timeline**
   permet de retimer les clés et le **Graphe** de tracer les canaux X/Y/Z,
   vitesse et roulis.
2. **Réglages** — durée, images/s, nombre d'images, FOV, profondeur de champ,
   roulis, liaison caméra et contraintes sol/parois. Un encadré indique la
   vitesse demandée par rapport à la croisière du profil et propose une durée
   conseillée.
3. **Profil** — cartes drone / oiseau / hélicoptère / dirigeable / chasse /
   manuel, caractère du corps, courbes vitesse et roulis comparées, trajectoires
   superposées et bouton **Tracé de démonstration (S)** pour comparer en
   quelques secondes. Le profil change la dynamique, pas le tracé.
4. **Lecture** — lecture/pause, boucle, vitesse, vue design (le frustum s'anime
   sans quitter l'orbite) ou vue caméra, rendu d'une image et export de la
   séquence PNG.

Les **Réglages** affichent maintenant le timing segment par segment
(longueur réelle, durée, vitesse, images) avec deux actions : répartir les temps
par distance et appliquer une vitesse cible. Le panneau cible propose **Fixe**,
**Avance** (conservé), **Décalage rigide**, **Retard**, **Tracé animé** et
**Manuel**, avec copie/synchronisation du tracé de l'œil.

La **barre de lecture** reste ancrée en bas du viewport dans tous les onglets :
`Lire`, curseur de temps, image précédente/suivante, boucle, bascule
Design/Caméra et **Timeline**. `Espace` fait la même chose ; `F1` ouvre l'aide.
La durée étire les clés en temps par défaut (case **Étirer les clés avec la
durée**) : le tracé occupe tout le clip. Décochez-la pour garder des temps
absolus. Le suivi **exact** est actif par défaut (case **Suivre exactement le
tracé**) : la caméra est sur la courbe aux temps des clés et la vitesse
correspond à distance/durée. Décochez-le pour retrouver l'inertie du profil ;
un bandeau signale alors l'écart et propose d'allonger la durée ou de passer en
Manuel.

Le CLI expose les deux modèles :

```bash
# Rig édité dans l'UI : une image ou une séquence
vistaforge rig info scene.vfproj
vistaforge anim still scene.vfproj frame.png --time 2 --width 1920 --height 1080
vistaforge anim render scene.vfproj frames/ --fps 24 --width 1280 --height 720

# Trajectoire seule, sans géométrie (Blender, autre projet, tableur)
vistaforge rig export scene.vfproj survol.glb
vistaforge rig export scene.vfproj survol.vfcam
vistaforge rig import autre_terrain.vfproj survol.vfcam --out rejoue.vfproj

# Ancien modèle de vol par trajectoires générées (toujours disponible en CLI)
vistaforge anim render scene.vfproj frames/ --preset cinematic --seconds 4 \
  --fps 12 --shutter 180 --blur-samples 4 --video vol.mp4
```

Presets de l'ancien modèle : `cinelifter`, `freestyle`, `racing`,
`helicopter`, `cinematic`, `dive`. Trajectoires : orbite, circuit, crête,
piqué. `--shutter 180` accumule 4 sous-images par frame : c'est un vrai flou de
mouvement, pas un filtre.

Détails du modèle de rig : [`RIG.md`](RIG.md). Parcours UI et captures :
[`ANIMATION.md`](ANIMATION.md).

## Projets `.vfproj`

Un projet est une archive ZIP contenant :

```
project.ron                 manifeste versionné (scène, source, réglages)
terrain/heightmap.r32       f32 little-endian, width*height
terrain/mask.u8             masque de validité optionnel
preview.png                 vignette optionnelle
```

Commandes :

```bash
vistaforge project info scene.vfproj
vistaforge project info scene.vfproj --json
vistaforge terrain info scene.vfproj
vistaforge terrain resample scene.vfproj --width 1026 --filter bicubic
vistaforge terrain smooth scene.vfproj --radius 2 --iterations 1
vistaforge scene preset scene.vfproj sunset
vistaforge scene preset scene.vfproj desert --out desert.vfproj
```

Le format préserve les réglages de caméra, soleil, palette, eau, nuages,
brume, rendu et **pile de modificateurs**. Le schéma courant est **v6** ; la
migration accepte v1→v6 (les champs absents prennent leur valeur par défaut).

## Heightmaps, GeoTIFF et DEM

L'espace **Fichiers** (et le CLI) importe un terrain depuis un GeoTIFF, un
ESRI ASCII Grid, un DEM v1 ou une heightmap PNG/JPEG.

```bash
# Heightmap 8 bits : plage d'altitude explicite
vistaforge terrain import relief.png --out relief.vfproj \
  --min-height 120 --max-height 900 --spacing 25

# Heightmap 16 bits avec sidecar JSON : plage automatique
vistaforge terrain import heightmap_16.png --out relief.vfproj

# GeoTIFF : géoréférencement lu dans le fichier
vistaforge terrain import dem.tif --out dem.vfproj

# Export terrain (l'extension choisit le format)
vistaforge terrain export scene.vfproj dem.tif       # GeoTIFF float géoréférencé
vistaforge terrain export scene.vfproj dem.asc       # ESRI ASCII Grid
vistaforge terrain export scene.vfproj heightmap.png # PNG 16 bits + JSON + PGW
vistaforge terrain export scene.vfproj apercu.jpg    # JPEG (lossy)
vistaforge terrain export scene.vfproj ancien.dem    # DEM v1
vistaforge terrain export scene.vfproj brut.r32      # f32 + sidecar JSON
```

Règles : `--min-height/--max-height` à `0/0` signifie automatique (sidecar
JSON). Les images sont nord en haut ; l'import les retourne pour la convention
VistaForge (ligne 0 au sud). Le GeoTIFF écrit est un float32 simple bande avec
`ModelPixelScale`, `ModelTiepoint`, CRS projeté utilisateur en mètres et
`GDAL_NODATA`. Détails et schéma du sidecar : [`FICHIERS.md`](FICHIERS.md).

## DEM legacy

```bash
vistaforge dem info re/unpacked/disk03/ALPS.DE
vistaforge dem info re/unpacked/disk03/ALPS.DE --json
vistaforge dem roundtrip re/unpacked/disk03/ALPS.DE
vistaforge dem preview re/unpacked/disk03/ALPS.DE alpes-hillshade.png
vistaforge dem convert re/unpacked/disk03/ALPS.DE copie.dem
vistaforge dem convert re/unpacked/disk03/ALPS.DE grille.asc --ascii

vistaforge terrain import re/unpacked/disk03/ALPS.DE --out alpes.vfproj
vistaforge render alpes.vfproj alpes.png --quality high
```

Le lecteur valide l'en-tête, les dimensions, le RLE, les deltas et refuse les
fichiers tronqués avec un message contextuel. L'import DEM désactive l'océan
global par défaut ; activez-le avec `water.set` via MCP ou en éditant le projet.

## Grands mondes et tuilage

```bash
cargo run --release -p vf-cli -- terrain world ile.vfproj --mode seamless \
  --tiles-x 3 --tiles-y 3 --blend 48 --macro 55 --apply --out ile_3x3.vfproj
cargo run --release -p vf-cli -- render ile_3x3.vfproj grand.png --mode photoreal
```

Détails, garanties et limites : [`MONDE.md`](MONDE.md).

## Modes de rendu

`--mode fast|refined|photoreal` règle SSAA, ombres, AO et subdivision.
`photoreal` reste déterministe ; le chemin GPU/photoréaliste est Blender.

## Rig caméra

```bash
cargo run --release -p vf-cli -- rig preset ile.vfproj drone --path orbit
cargo run --release -p vf-cli -- rig info ile.vfproj
cargo run --release -p vf-cli -- anim still ile.vfproj still.png --time 2.4
cargo run --release -p vf-cli -- anim render ile.vfproj frames/ --seconds 8 --fps 24
```

Détails et presets : [`RIG.md`](RIG.md).

## Bridge Blender

```bash
cargo run --release -p vf-cli -- blender export ile.vfproj demo-out/blender/ile --subdivision 2
cargo run --release -p vf-cli -- blender render ile.vfproj demo-out/blender/ile.png --samples 64

# Trajectoire caméra/cible seule, pour l'addon Blender
cargo run --release -p vf-cli -- rig export ile.vfproj demo-out/blender/ile/camera_animation.glb
```

L'espace Fichiers > Blender écrit `scene.glb`, les textures, `scene.json` et
optionnellement `camera_animation.glb`. L'addon
`bridge/blender/vistaforge_io.py` importe la trajectoire, sélectionne la caméra,
règle fps/plage d'images et réexporte un `.vfcam` vers VistaForge. Voir
[`BLENDER.md`](BLENDER.md) et [`FICHIERS.md`](FICHIERS.md).

Détails, contenu du dossier et limites : [`BLENDER.md`](BLENDER.md).

## Interface de bureau

`vistaforge-app [projet.vfproj]` ouvre :

- un **viewport** 3D avec orbite à la souris, zoom à la molette et gizmo CAO ;
- une **barre de fonctions** : Terrain, Modificateurs, Roche, Détail, Sculpter,
  Animation, Érosion, Eau, Matières, Lumière, Caméra, Rendu. Chaque famille
  ouvre un panneau latéral ; **Animation** contient le tracé, la caméra et le
  vol, sans onglet Vol séparé ;
- des lignes de réglage uniformes : libellé, slider, valeur numérique éditable
  (double-clic pour revenir au défaut) ;
- le panneau **Modificateurs** (pile, paramètres générés, cuisson) ;
- l'espace **Animation** avec ses quatre onglets Tracé / Réglages / Profil /
  Lecture, le gizmo de tracé, le viseur du point de vue et la barre de lecture ;
- l'**aide contextuelle** dans le viewport : survolez un espace, une section ou
  un réglage, la fiche correspondante s'ouvre avec une capture annotée ;
- le panneau **Rendu** avec la bascule viewport GPU, les contrôles d'aperçu
  adaptatif et le rendu final.

### Navigation 3D et gizmo

Le gizmo d'orientation, en haut à droite du viewport, reprend la disposition
d'une triade CAO :

- le **cube** est solidaire de la caméra ; les faces `FACE`, `DOS`, `GAUCHE`,
  `DROITE`, `HAUT`, `BAS` sont cliquables (seules les faces visibles le sont) ;
- les **axes X/Y/Z** (rouge/vert/bleu) et leurs extrémités négatives sont
  cliquables : un clic place l'œil sur cet axe et cadre le terrain ;
- le bouton **Défaut** (maison) revient à la vue d'ensemble en perspective ;
- un **double-clic** dans le viewport fait la même chose que le bouton Défaut ;
- raccourcis clavier : `1` face, `2` dessus, `3` droite, `0` ou `Home` défaut.

Les vues restent des caméras perspective (le rasteriseur CPU n'a pas de
projection orthographique), mais l'œil est exactement sur l'axe et le cadrage
couvre le terrain. Le panneau **Caméra** expose les mêmes vues en boutons, les
angles yaw/pitch, la distance et le FOV. Le gizmo peut être masqué par la case
**Gizmo d'orientation**.

### Aperçu adaptatif et viewport GPU

Deux chemins d'affichage coexistent, sans changer le rendu final :

1. **Viewport GPU (wgpu)** — quand un adaptateur est disponible, le viewport
   est dessiné en temps réel par le GPU (Vulkan sous Linux/Windows, Metal sur
   macOS). Un maillage du heightfield (jusqu'à 2 049 segments par côté) est
   construit avec la pile de modificateurs et le pass falaises, puis éclairé
   par le soleil, l'eau et le brouillard. C'est le mode par défaut.
2. **Aperçu CPU adaptatif** — si le GPU est indisponible, désactivé dans le
   panneau **Rendu**, ou si l'on force `VISTAFORGE_RENDERER=glow`, l'aperçu
   est rendu sur le CPU. Sa résolution part de 25–50 % de 960×540 pendant
   l'interaction, puis se raffine (≈ 50 → 75 → 100 %) après ~200 ms
   d'inactivité. L'échelle interactive est réapprise à partir du temps de
   rendu mesuré pour viser ~45 ms par passe. Le HUD et le panneau **Rendu**
   affichent la résolution et le temps de la dernière passe.

Le bouton **Render** lance toujours le rendu final CPU déterministe, à la
résolution de sortie. Une fois terminé, son image remplace le viewport ;
le prochain mouvement de caméra ou changement de scène rebascule sur la vue
GPU. Le bouton **Revenir à la vue GPU** (panneau Rendu) le fait immédiatement.

Le viewport GPU est volontairement plus léger que le rendu final : pas
d'ombres portées, d'AO, de nuages, de détail de roche ni de post-traitement.
La pile de modificateurs, les falaises, l'eau, le soleil, la palette et
l'exposition sont appliqués. Voir [`GPU.md`](GPU.md) pour l'architecture et
les limites.

En cas d'absence de pilote wgpu, lancer l'interface avec
`VISTAFORGE_RENDERER=glow` : le rendu CPU et l'aperçu adaptatif restent
disponibles. **Open/Save** utilisent le chemin affiché. La séquence de vol
complète se rend en CLI (`anim render`).

## Serveur MCP

```bash
vistaforge-mcp --workspace /chemin/vers/workspace
```

Le serveur parle JSON-RPC 2.0 sur stdin/stdout (une ligne par message). Les
logs vont sur stderr. Tous les chemins sont relatifs au workspace et confinés ;
les chemins absolus et `..` sont refusés.

Exemple de configuration client :

```json
{
  "mcpServers": {
    "vistaforge": {
      "command": "/chemin/vers/vistaforge-mcp",
      "args": ["--workspace", "/chemin/vers/workspace"]
    }
  }
}
```

Guide complet pour un clone sur un autre système (Linux précompilé, macOS,
Windows, Claude Desktop, Claude Code, Cursor, VS Code, Inspector) :
[`MCP.md`](MCP.md).

Outils exposés :

| Outil | Rôle |
|---|---|
| `project.new`, `project.open`, `project.save`, `project.info`, `project.list` | cycle de vie du projet |
| `terrain.generate`, `terrain.import_dem`, `terrain.resample`, `terrain.smooth` | génération et édition |
| `geology.set` | applique un preset de roche et reconstruit le terrain |
| `terrain.height_query`, `terrain.export_dem`, `terrain.export_ascii` | lecture et export (pile appliquée) |
| `modifiers.kinds`, `modifiers.list`, `modifiers.add`, `modifiers.set`, `modifiers.remove`, `modifiers.clear`, `modifiers.bake` | pile de modificateurs |
| `flight.info`, `flight.still` | simule un vol et rend une image |
| `scene.preset`, `camera.set`, `sun.set`, `water.set`, `atmosphere.set` | scène (dont `dusk` et les étoiles) |
| `render.still`, `render.preview` | rendu PNG (aperçu renvoyé en image MCP) |

Les erreurs d'outil sont renvoyées avec `isError: true` et un texte explicite,
sans interrompre la session.

## Déterminisme

- Même projet + même seed + même version ⇒ mêmes hauteurs et mêmes masques.
- RNG SplitMix64 interne, aucun `thread_rng`.
- Le rendu CPU ne dépend ni du GPU ni de l'ordre des threads ; le viewport GPU
  est un aperçu interactif et n'intervient jamais dans l'image exportée.
- Les seeds dérivés sont nommés : `seed.derive("erosion")`, `"render"`, etc.

## Limites de la version 0.2

- Animation : le tracé est un chemin Bézier par clés ; les poignées manuelles
  ne sont pas encore éditables dans le viewport. L'ancien modèle de vol par
  trajectoires générées reste disponible en CLI mais n'a plus d'onglet dédié.
- Pas de végétation instanciée ni d'ombres de nuages volumétriques.
- Le rendu est un rasteriseur CPU : pas de path tracing.
- Undo/redo limité au sculpteur ; pas d'historique global du projet.
- Surplombs et arches impossibles en heightfield : le pass `cliffs` produit des
  parois verticales, pas des surplombs. Le modificateur `recast_cliff` lisse la
  paroi mais reste un heightfield.
- La crête d'une falaise peut garder une dentelure sur un relief très découpé.
- Les modificateurs sont appliqués au rendu : les statistiques du panneau UI
  restent celles du terrain de base ; `terrain info --json` donne les
  statistiques modifiées.
