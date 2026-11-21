# Binaires précompilés — Windows x86_64

Ce dossier contient les binaires **Windows x86_64** de VistaForge, construits
depuis la source de ce dépôt avec `cargo build --release -p vf-cli -p vf-mcp
-p vf-app`.

| Binaire | Rôle | Taille |
|---|---|---|
| `vistaforge.exe` | CLI : génération, import/export DEM et heightmaps, rendu, animation, Blender, MCP tools. | 5,4 Mo |
| `vistaforge-app.exe` | Interface graphique `egui` + viewport GPU wgpu (Vulkan/DX12) et aperçu CPU. | 21,9 Mo |
| `vistaforge-mcp.exe` | Serveur MCP stdio (29 outils). | 4,2 Mo |

Construits le 22 novembre 2026 depuis l'arbre de travail courant
(commit `7f294f5`, interface GPU/CPU unifiée, tafoni/bancs/joints/oolithes,
presets rocheux enrichis, résolutions 2048–8192). Toolchain épinglée
`1.99.1-x86_64-pc-windows-msvc`, profil `release` (`lto = "thin"`,
`codegen-units = 1`, `panic = "abort"`).

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

Le viewport GPU utilise wgpu (Vulkan, repli DX12) par défaut. Sans pilote
compatible :

```powershell
$env:VISTAFORGE_RENDERER = "glow"; .\vistaforge-app.exe
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
