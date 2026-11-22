# Binaires précompilés — Windows x86_64

Ce dossier contient les binaires **Windows x86_64** de VistaForge **0.2.0**,
construits depuis la source de ce dépôt avec `cargo build --release -p vf-cli
-p vf-mcp -p vf-app`.

| Binaire | Rôle | Taille |
|---|---|---|
| `vistaforge.exe` | CLI : génération, import/export DEM et heightmaps, rendu, animation, Blender, **maillage 3D** (`mesh`), MCP tools. | 10,7 Mo |
| `vistaforge-app.exe` | Interface graphique `egui`, modules **Heightmap / Maillage 3D / Animation / Scène / Fichiers**, viewport GPU wgpu (Vulkan/DX12) et aperçu CPU, moteur de calcul (CPU N cœurs / un GPU / tous les GPU). | 23,0 Mo |
| `vistaforge-mcp.exe` | Serveur MCP stdio (29 outils). | 4,5 Mo |

Construits le 22 novembre 2026 depuis `master`, release `v0.2.0` : module
**Maillage 3D** (sculpture, remesh quads/voxel, fracture, physique, parois,
export GLB/OBJ/STL/VFM), **passage à sens unique heightmap → maillage** (le
maillage reprend la finition complète du rendu à la résolution du heightmap),
gizmo d'orientation, **terrain initial neutre** (aucun bruit ni modificateur par
défaut), modules et sous-onglets cohérents (Roche + Détail fusionnés), et côté
animation : **angles par clé**, **décalage de poursuite** intérieur/extérieur/
haut/bas constant ou par keyframe, canal accélération avec limites et boutons
±10 % par segment. Interface GPU/CPU unifiée, tafoni/bancs/joints/oolithes,
presets rocheux enrichis, résolutions 2048–8192.
La CLI embarque le moteur GPU (`vf-compute`), d'où sa taille plus élevée : la
bibliothèque `wgpu` est liée statiquement et les pilotes sont chargés à
l'exécution.
Toolchain épinglée `1.99.1-x86_64-pc-windows-msvc`, profil `release`
(`lto = "thin"`, `codegen-units = 1`, `panic = "abort"`).

## Utilisation

```powershell
cd dist\windows-x86_64
.\vistaforge.exe --version
# vistaforge 0.2.0
.\vistaforge.exe --help

# Interface
.\vistaforge-app.exe

# Serveur MCP
.\vistaforge-mcp.exe --workspace C:\chemin\vers\projets
```

Maillage 3D depuis le CLI :

```powershell
.\vistaforge.exe mesh --project demo\canyon.vfproj --source wall `
  --wall sandstone-canyon --rock canyon --out paroi.glb --render paroi.png
```

Le viewport GPU utilise wgpu (Vulkan, repli DX12) par défaut. Sans pilote
compatible :

```powershell
$env:VISTAFORGE_RENDERER = "glow"; .\vistaforge-app.exe
```

## Budget de calcul (CPU / GPU)

Dans l'interface, **Préférences → Calcul (CPU / GPU)** règle deux choses :

- le **moteur des transformations géométriques** (normales, lissage de Taubin,
  déplacement du détail, transformations, sculpture) : pool CPU de 1 à N cœurs,
  un adaptateur GPU, ou tous les GPU détectés. Le changement est **immédiat** et
  se retrouve aussi dans le sélecteur **Calcul** du panneau Maillage 3D ;
- le **pool de threads partagé** : génération, érosion, modificateurs, géologie,
  maillage, fracture, voxel, aperçus et rendu final. Il s'applique sans
  redémarrer, et le résultat ne dépend pas du nombre de cœurs.

Sur un poste récent, le CPU est plus rapide que le GPU pour ces noyaux (chaque
appel GPU transfère la topologie) ; le GPU aide quand peu de cœurs sont alloués
ou sur une machine à peu de cœurs. Mesures dans
[`docs/GPU.md`](../../docs/GPU.md#cpu-ou-gpu--ce-que-disent-les-mesures).

```powershell
# Limiter la CLI à 8 threads (le résultat est identique)
.\vistaforge.exe --threads 8 terrain generate --mode modern --size 2048 --erode --out monde.vfproj

# Forcer le GPU (ou tous les GPU) pour les calculs de maillage
.\vistaforge.exe --compute gpu mesh --project demo\canyon.vfproj --resolution 160 --out canyon.glb
.\vistaforge.exe --compute all --verbose mesh --project demo\canyon.vfproj --resolution 160 --out canyon.glb

# Mêmes réglages par variable d'environnement, pour l'interface aussi
$env:VISTAFORGE_THREADS = "8"
$env:VISTAFORGE_COMPUTE = "gpu"                    # ou "cpu", "all"
$env:VISTAFORGE_COMPUTE_GPU = "NVIDIA GeForce RTX 4070"
$env:VISTAFORGE_GPU = "NVIDIA GeForce RTX 4070"   # aperçu 3D, ou "auto", "off"
.\vistaforge-app.exe
```

## Prérequis

- Windows 10/11 x86_64.
- Pour l'interface : pilote Vulkan ou DX12 (recommandé) ou OpenGL 3.3.
- Les fonctions DEM/heightmap et le rendu CPU ne demandent aucune
  bibliothèque externe.
- Les binaires sont liés dynamiquement à la bibliothèque d'exécution Visual
  C++ (`vcruntime140.dll`, `msvcp140.dll`). Elle est présente sur la plupart
  des installations ; sinon, installer le paquet
  [Microsoft Visual C++ Redistributable x64](https://aka.ms/vs/17/release/vc_redist.x64.exe).

## Vérifier l'intégrité

```powershell
# PowerShell natif
Get-FileHash vistaforge.exe, vistaforge-app.exe, vistaforge-mcp.exe -Algorithm SHA256

# ou, dans Git Bash
sha256sum -c SHA256SUMS
```

## Notes

- Ces binaires sont produits localement, sans signature de code : SmartScreen
  peut demander une confirmation au premier lancement.
- Les binaires ne sont pas `strip`és (pas de `strip = "symbols"` dans le
  profil release du dépôt) ; c'est pourquoi ils pèsent quelques Mo de plus
  que strictement nécessaire.
- Les captures, projets `.vfproj` et corpus de recherche ne sont pas inclus.
- Pour macOS ou Linux, recompiler depuis la source (`cargo build --release
  -p vf-app -p vf-cli -p vf-mcp`).
