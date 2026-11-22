# Binaires précompilés

Ce dossier contient les binaires **précompilés** de VistaForge, construits
depuis la source de ce dépôt avec `cargo build --release` :

| Plateforme | Dossier | Compilation |
|---|---|---|
| Windows x86_64 | [`windows-x86_64/`](windows-x86_64/README.md) | Rust 1.99.1 MSVC, version 0.2.0 |

Les binaires **Linux ne sont plus publiés** : ceux du dépôt avaient pris du
retard sur le code (module Maillage 3D, moteur de calcul CPU/GPU) et une version
périmée vaut moins qu'une compilation locale. Sur Linux et macOS, recompilez
depuis la source — la toolchain est épinglée et s'installe automatiquement :

```bash
git clone https://github.com/vingaut/vistaforge.git
cd vistaforge
cargo build --release -p vf-cli -p vf-mcp -p vf-app

./target/release/vistaforge --version
./target/release/vistaforge-app
./target/release/vistaforge-mcp --workspace /chemin/vers/projets
```

Chaque dossier livré contient les trois exécutables et un `SHA256SUMS` :

| Binaire | Rôle |
|---|---|
| `vistaforge` | CLI : génération, import/export DEM et heightmaps, rendu, animation, Blender, maillage 3D (`mesh`). |
| `vistaforge-app` | Interface graphique `egui` + viewport GPU wgpu (Vulkan/DX12) et aperçu CPU. |
| `vistaforge-mcp` | Serveur MCP stdio (29 outils). |

Tailles Windows (non `strip`és) : CLI 10,6 Mo, interface 22,9 Mo, serveur MCP
4,4 Mo. Les binaires Windows ont été reconstruits le 22 novembre 2026
(version 0.2.0) depuis l'arbre de travail courant : module **Maillage 3D**,
gizmo d'orientation, moteur de calcul au choix (CPU N cœurs / un GPU / tous les
GPU), remesh déterministe, interface GPU/CPU unifiée, presets rocheux enrichis
et résolutions 2048–8192. La CLI embarque `wgpu` pour le calcul GPU, d'où sa
taille.

## Utilisation

```powershell
cd dist\windows-x86_64
.\vistaforge.exe --version
.\vistaforge.exe --help

# Interface
.\vistaforge-app.exe

# Serveur MCP
.\vistaforge-mcp.exe --workspace C:\chemin\vers\projets
```

Le viewport GPU utilise wgpu (Vulkan, repli DX12 sous Windows) par défaut.
Sans pilote compatible :

```powershell
$env:VISTAFORGE_RENDERER = "glow"; .\vistaforge-app.exe

# Même chose sous Linux/macOS après compilation depuis la source :
VISTAFORGE_RENDERER=glow ./target/release/vistaforge-app
```

## Prérequis

- Windows 10/11 x86_64 (binaires MSVC ; `vcruntime140.dll` requis).
- Pour l'interface : pilote Vulkan (recommandé), DX12 ou OpenGL.
- Linux/macOS depuis la source : voir [`docs/BUILD.md`](../docs/BUILD.md)
  (X11 ou Wayland, `libvulkan1` + pilote Vulkan recommandés).
- Les fonctions DEM/heightmap et le rendu CPU ne demandent aucune bibliothèque
  externe.

## Vérifier l'intégrité

```powershell
Get-FileHash * -Algorithm SHA256   # PowerShell

sha256sum -c SHA256SUMS            # Git Bash sous Windows
```

## Notes

- Ces binaires sont produits localement, sans signature de code.
- Les captures, projets `.vfproj` et corpus de recherche ne sont pas inclus.
- Les prérequis, le budget de calcul et les variables d'environnement sont
  détaillés dans [`windows-x86_64/README.md`](windows-x86_64/README.md).
