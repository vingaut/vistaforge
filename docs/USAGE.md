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

## Vol et animation

Le module de vol simule une caméra de drone FPV ou d'hélicoptère le long d'une
trajectoire générée sur le terrain. La gîte vient de l'accélération
centripète réelle, l'attitude est lissée, et des secousses déterministes
ajoutent le grain du vol.

```bash
# Une image fixe du vol, à t = 2 s
vistaforge anim still scene.vfproj frame.png \
  --preset cinematic --time 2 --width 1920 --height 1080

# Une séquence PNG de 4 s à 12 images/s, avec flou de mouvement à 180°
vistaforge anim render scene.vfproj frames/ \
  --preset cinematic --seconds 4 --fps 12 \
  --width 1280 --height 720 --shutter 180 --blur-samples 4

# Encodage MP4 si ffmpeg est installé (sinon la commande à lancer est affichée)
vistaforge anim render scene.vfproj frames/ --video vol.mp4
```

Presets : `cinelifter`, `freestyle`, `racing`, `helicopter`, `cinematic`,
`dive`. Trajectoires : orbite, circuit, crête, piqué. `--shutter 180` accumule
4 sous-images par frame : c'est un vrai flou de mouvement, pas un filtre.

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
migration accepte v1→v5 (les champs absents prennent leur valeur par défaut).

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
```

Détails, contenu du dossier et limites : [`BLENDER.md`](BLENDER.md).

## Interface de bureau

`vistaforge-app [projet.vfproj]` ouvre :

- un **viewport** 3D avec orbite à la souris et zoom à la molette ;
- une **barre de fonctions** : Terrain, Modificateurs, Roche, Érosion, Eau,
  Matières, Lumière, Caméra, Vol, Rendu. Chaque famille ouvre un panneau
  latéral ;
- des lignes de réglage uniformes : libellé, slider, valeur numérique éditable
  (double-clic pour revenir au défaut) ;
- le panneau **Modificateurs** (pile, paramètres générés, cuisson) ;
- le panneau **Vol** avec presets, trajectoire, dynamique et aperçu à un temps
  donné.

Les modifications déclenchent un aperçu 960×540 débouncé ; le bouton
**Render** produit l'image finale. **Open/Save** utilisent le chemin affiché.
La séquence de vol complète se rend en CLI (`anim render`).

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
- Le rendu CPU ne dépend ni du GPU ni de l'ordre des threads.
- Les seeds dérivés sont nommés : `seed.derive("erosion")`, `"render"`, etc.

## Limites de la version 0.2

- Vol par trajectoires générées (orbite, circuit, crête, piqué) ; pas encore de
  waypoints libres ni de script `.SCR`.
- Pas de bridge Blender/glTF.
- Pas de végétation instanciée ni d'ombres de nuages volumétriques.
- Le rendu est un rasteriseur CPU : pas de path tracing.
- Pas d'undo/redo ; l'UI reste compacte.
- Surplombs et arches impossibles en heightfield : le pass `cliffs` produit des
  parois verticales, pas des surplombs. Le modificateur `recast_cliff` lisse la
  paroi mais reste un heightfield.
- La crête d'une falaise peut garder une dentelure sur un relief très découpé.
- Les modificateurs sont appliqués au rendu : les statistiques du panneau UI
  restent celles du terrain de base ; `terrain info --json` donne les
  statistiques modifiées.
