# Binaires précompilés

Ce dossier contient les binaires **Linux x86_64** de VistaForge, construits
depuis la source de ce dépôt avec `cargo build --release --workspace`.

| Binaire | Rôle |
|---|---|
| `vistaforge` | CLI : génération, import/export DEM et heightmaps, rendu, animation, Blender, MCP tools. |
| `vistaforge-app` | Interface graphique `egui` + viewport GPU wgpu (Vulkan) et aperçu CPU. |
| `vistaforge-mcp` | Serveur MCP stdio (29 outils). |

Tailles : CLI 5,8 Mo, interface 24 Mo, serveur MCP 4,6 Mo. Binaires
`strip`és, construits depuis la source au commit `859104a` avec
`cargo build --release --workspace` (Rust 1.99.1).

## Utilisation

```bash
cd dist/linux-x86_64
./vistaforge --version
./vistaforge --help

# Interface
./vistaforge-app

# Serveur MCP
./vistaforge-mcp --workspace /chemin/vers/projets
```

Le viewport GPU utilise Vulkan par défaut. Sans pilote Vulkan compatible :

```bash
VISTAFORGE_RENDERER=glow ./vistaforge-app
```

## Prérequis

- Linux x86_64, glibc ≥ 2.39 (construit sur Ubuntu 24.04, glibc 2.43).
- Pour l'interface : pilote Vulkan (recommandé) ou OpenGL, X11 ou Wayland.
- Les fonctions DEM/heightmap ne demandent aucune bibliothèque externe.

## Vérifier l'intégrité

```bash
sha256sum -c SHA256SUMS
```

## Notes

- Ces binaires sont produits localement, sans signature de code.
- Les captures, projets `.vfproj` et corpus de recherche ne sont pas inclus.
- Pour macOS/Windows, recompiler depuis la source (`cargo build --release -p vf-app -p vf-cli -p vf-mcp`).
