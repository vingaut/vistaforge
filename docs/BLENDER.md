# Bridge Blender

VistaForge exporte la scène **telle que le rasteriseur la voit** — modificateurs,
pass de falaises et détail continu compris — vers un GLB, des textures cuites
et un script d'import Blender. Le but est de continuer le texturing dans
Blender sans reconstruire la géométrie ni deviner les matériaux.

![Rendu Cycles de l'export alpine](screenshots/blender_cycles.png)

---

## 1. Commandes

```bash
# Export seul : écrit scene.glb, scene.json, textures et un README
cargo run --release -p vf-cli -- blender export ile.vfproj demo-out/blender/ile \
  --subdivision 2

# Export + import + rendu Cycles headless
cargo run --release -p vf-cli -- blender render ile.vfproj demo-out/blender/ile.png \
  --samples 64 --width 1600 --height 900

# GPU (OptiX/CUDA si disponible)
cargo run --release -p vf-cli -- blender render ile.vfproj demo-out/blender/ile_gpu.png \
  --samples 128 --gpu

# Import dans une session Blender interactive :
# Scripting > Open > bridge/blender/vistaforge_import.py, puis renseigner glb et json.
```

Options utiles : `--no-water`, `--no-camera`, `--no-sun` à l'export ;
`--script`, `--blender`, `--out-dir` au rendu. Le script accepte aussi
`--validate` (import sans rendu), `--smart-uv`, `--remesh` et
`--save-blend`.

---

## 2. Contenu du dossier d'export

| Fichier | Contenu |
|---|---|
| `scene.glb` | maillage réel (détail continu inclus), normales, UV0 plan et UV1 métrique, couleurs de sommets, 7 emplacements de matériaux, caméra, soleil. |
| `scene.json` | palette, soleil, caméra, unités, CRS, stats du terrain, réglages de rendu, roche, détail, correspondance des matériaux. |
| `albedo.png` | albédo cuit en UV0 (sRGB), à remplacer par les textures de l'artiste. |
| `normal.png` | normale dérivée du heightfield détaillé (UV0, espace top-down). |
| `masks.png` | R = neige, G = végétation, B = éboulis, pour mélanger des textures. |
| `heightmap_16.png` | heightfield de la surface exportée, 16 bits, ligne 0 = sud. |
| `README.txt` | rappel de la commande d'import. |

### Coordonnées

VistaForge travaille en **Z-up, mètres**. glTF est **Y-up** ; l'export applique
`(x, y, z) -> (x, z, -y)`. La caméra regarde son -Z local, le soleil voyage
selon son -Z local. Les unités restent métriques dans Blender.

### Matériaux

Sept emplacements sont écrits :

| Matériau | Rôle |
|---|---|
| `vf_rock` | roche de fond, la plupart des faces. |
| `vf_strata_a`, `vf_strata_b` | bancs stratifiés (si la géologie est active). |
| `vf_scree` | éboulis. |
| `vf_snow` | neige. |
| `vf_vegetation` | végétation basse. |
| `vf_water` | plan de mer. |

Le script Blender branche `albedo.png` sur la couleur de base, `normal.png`
sur la normale, et ajoute un bruit procédural de secours. Il expose aussi
`masks.png` dans le graphe de nœuds : l'artiste peut remplacer l'albédo par
ses propres textures par matériau et utiliser les masques pour les mélanges.

### UV

- **UV0** : projection planaire du terrain (une seule charte). C'est celle qui
  reçoit les textures cuites.
- **UV1** : **atlas de boîte** à six projections (3×2 cellules), pour les
  falaises et les surplombs. Les arêtes entre deux projections peuvent être
  recousues dans Blender (Merge by Distance) ou remplacées par `--smart-uv`.
- Le GLB importe bien **deux couches UV** (vérifié : `2 UV layer(s)`).

`--smart-uv` recalcule une projection dans Blender, au prix de la
correspondance avec `albedo.png` et `normal.png`. `--remesh` applique un
modifier Voxel Remesh destructif ; à réserver aux objets lourds ou aux tests.
`--save-blend` écrit un `.blend` configuré avant le rendu.

---

## 3. Pipeline exporté

```
heightfield stocké
  → pile de modificateurs (si active)
  → pass falaises (cliff_warp inclus)
  → champ de détail continu (profil de roche)
  → maillage subdivisé (1–8×, budget de sommets)
  → couleurs de sommets + classes de matériaux
  → GLB + textures cuites + sidecar
```

C'est le même ordre que le rendu CPU. Un modificateur ou un réglage de falaise
qui n'apparaît pas dans Blender est un bug.

---

## 4. Validation

Validé sur cette machine avec **Blender 5.3** :

```text
VistaForge import: 1 mesh object(s), 1054733 vertices, 2 material(s), camera=True
VistaForge render written to docs/screenshots/blender_cycles.png
```

Le rendu Cycles 960×600, 24 échantillons, CPU, prend ~13 s (import compris).
Le GPU n'est pas activé par défaut car une session headless n'a pas toujours
de périphérique OptiX configuré ; `--gpu` tente OptiX et retombe sur CPU.

---

## 5. Limites de la version 1

- Export **aller seulement** : pas de round-trip Blender → VistaForge.
- Pas d'arbres/rochers instanciés ni de végétation géométrique.
- `albedo.png` et `normal.png` sont des textures de départ, pas des baking
  PBR multi-UDIM ; le script Smart UV/remesh est là pour les cas difficiles.
- L'atlas UV1 est par sommet : les triangles qui changent d'axe peuvent
  étirer une couture. Smart UV reste la solution propre pour un asset final.
- Les surplombs maillés (P1.5) ne sont pas encore exportés comme objets
  séparés ; `undercut` est cuit dans le heightfield.
- Le plan d'eau est un rectangle ; pas de rivière ni de lac vectoriel.
