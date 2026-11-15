# Viewport 3D GPU (wgpu / Vulkan)

VistaForge conserve un rendu final **CPU déterministe** pour les exports PNG et
le CLI. Le viewport de l'interface de bureau peut en plus être dessiné par le
GPU via `wgpu` : Vulkan sous Linux/Windows, Metal sur macOS, DX12/OpenGL en
repli. Ce document décrit ce chemin, ce qu'il montre et ce qu'il ne montre pas.

## Pourquoi un chemin séparé

Le rasteriseur CPU est la référence : ombres, AO, nuages, détail de roche,
eau avec écume et tone mapping ACES. Il n'est pas transposable tel quel dans un
shader sans réécrire tout le rendu. Le viewport GPU est donc un **aperçu de
navigation** : il doit être fluide pendant l'orbite, le zoom et les réglages,
pas pixel-identique au rendu final.

Le viewport GPU :

- applique la pile de modificateurs (`vf_modifiers`) et le pass falaises
  (`vf_procgen::cliff::sharpen`) avant de construire le maillage ;
- affiche un ciel dégradé avec soleil, un relief éclairé par le soleil, une
  eau opaque avec Fresnel/reflet, et du brouillard ;
- utilise la palette, l'azimut/déclinaison du soleil et l'exposition du projet ;
- respecte l'activation de l'eau (panneau Eau et case Water du panneau Rendu) ;
- ne montre pas les ombres portées, l'AO, les nuages, le détail de roche, la
  subdivision adaptative, les matériaux avancés ni le post-traitement.

Le rendu final (`Render`) reste produit par `vf-render` sur le CPU, à la
résolution de sortie, et devient l'image affichée jusqu'au prochain changement.

## Architecture

```
 UI (egui)                         render thread (eframe/wgpu)
 ─────────                         ───────────────────────────
 heightfield ──► build_terrain ──► vertex/index buffers
 project     ──► uniforms       ──► uniform buffer
 camera      ──► callback.prepare ─► offscreen color + Depth32Float
                                  └► sky pass, terrain pass
 callback.paint ─────────────────► blit texture dans le viewport egui
```

Points clés :

- **Cible offscreen** : le render pass d'egui n'a pas de depth buffer. Le
  callback rend donc le ciel et le terrain dans une texture couleur + depth,
  puis un triangle plein écran échantillonne cette texture dans le pass egui.
- **Callback egui-wgpu** : `GpuCallback` est ajouté au painter pour le rectangle
  du viewport. `prepare` encode le rendu offscreen dans l'encodeur d'egui ;
  `paint` blitte la texture. Aucun command buffer séparé n'est nécessaire.
- **Maillage** : échantillonnage du heightfield avec un pas calculé pour ne pas
  dépasser 1024 segments par côté (≈ 1 M de sommets au maximum). Les normales
  sont calculées par différences centrées. Une grande nappe d'eau est ajoutée
  au maillage lorsque l'eau est active.
- **Coordonnées** : les sommets et la caméra sont translatés par rapport au
  centre du terrain pour rester en `f32` avec une bonne précision.
- **Reconstruction** : le maillage GPU est reconstruit après un debounce de
  150 ms lorsqu'un réglage de scène change. Les changements de caméra seuls ne
  reconstruisent rien : le callback lit la caméra à chaque image.

## Shaders

- `src/shaders/scene.wgsl` : `vs_sky`/`fs_sky` (triangle plein écran + rayons),
  `vs_terrain`/`fs_terrain` (relief, eau, brouillard, exposition).
- `src/shaders/blit.wgsl` : copie de la texture offscreen vers le viewport.

Les couleurs de palette sont linéaires. Si la surface cible n'est pas sRGB, le
shader encode explicitement en sRGB ; sinon le GPU s'en charge à l'écriture.

## Bascule et dépannage

- Par défaut, l'application démarre avec `eframe::Renderer::Wgpu`.
- `VISTAFORGE_RENDERER=glow` force le renderer OpenGL d'egui et le viewport CPU.
- Dans le panneau **Rendu**, la case **Vue 3D GPU** active/désactive le
  viewport GPU. Le libellé indique l'adaptateur et le backend détectés
  (par exemple `NVIDIA … · Vulkan`).
- Si la création du viewport GPU échoue (shader, adaptateur, mémoire), l'erreur
  est affichée dans le panneau Rendu et l'aperçu CPU prend le relais.

## Limites connues

- Pas d'ombres, d'AO, de nuages ni de détail de roche dans le viewport GPU.
- Le maillage est reconstruit sur le thread UI (après debounce) : sur un très
  grand terrain ou une pile de modificateurs lourde, la synchronisation peut
  bloquer brièvement l'interface.
- Le viewport GPU n'est pas un chemin d'export : il ne modifie ni le PNG, ni le
  CLI, ni les résultats MCP.
- La lecture d'une séquence de vol complète reste un rendu CPU (CLI
  `anim render`) ; le GPU couvre l'exploration interactive du terrain.

## Pistes

- Pass d'ombres et d'AO GPU pour rapprocher l'aperçu du rendu final.
- Nuages 2.5D et eau transparente dans le shader.
- Reconstruction du maillage sur un worker pour les très grands heightfields.
- Rendu GPU du détail de roche (texture de détail ou compute).
