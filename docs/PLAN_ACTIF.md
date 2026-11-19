# Plan actif — session du 19 novembre 2026

> Ce fichier est le **plan de travail suivi**. Il est mis à jour après chaque
> étape validée (tests ou vérification visuelle). L'état détaillé des sessions
> précédentes reste dans [`ETAT_ET_REPRISE.md`](ETAT_ET_REPRISE.md).
>
> Convention : `[ ]` à faire, `[~]` en cours, `[x]` fait et vérifié.

## 1. Heightmap : contrôle du sens de la hauteur

- [x] Vérifier la convention d'altitude (image, `HeightField`, rendu CPU, GPU,
      export PNG/GeoTIFF, export Blender).
- [x] Corriger l'export `heightmap_16.png` du bridge Blender : il est écrit
      **ligne 0 = sud**, alors que tous les autres exports et l'UV0 sont
      nord-en-haut. Le même heightmap réinjecté en déplacement dans Blender
      donne donc des creux là où le terrain a des bosses.
- [x] Test de non-régression : colline lumineuse en haut → altitude haute au
      nord, rendu éclairé du côté du soleil.
- [x] Documenter la convention (nord en haut, blanc = haut, mètres).

## 2. Animation

- [x] **Cible optionnelle** : mode `Aucune` (la caméra regarde le long de la
      trajectoire, plus de gizmo cible).
- [x] **Avance exacte sur la courbe** : `Avance` échantillonne la courbe de la
      caméra à `t + avance`, au lieu d'extrapoler la vitesse. La cible reste
      donc exactement sur le tracé prévu, avec le décalage temporel demandé.
- [x] **Keyframing caméra / cible différencié** : deux pistes éditables
      clairement séparées, plus de troisième objet « œil » dans l'interface.
      L'œil interne devient le tracé caméra ; le gizmo et les libellés parlent
      de la caméra.
- [x] **Altitude relative au sol** : mode où la hauteur des clés est une
      hauteur **au-dessus du sol** (AGL), avec :
      - hauteur AGL caméra et cible ;
      - amortissement vertical caméra et cible (temps de réponse réglable) ;
      - anticipation du relief et plancher dur anti-pénétration.
- [x] Tests : cible sur la courbe, aucune cible, AGL plat + colline, damping,
      déterminisme.

## 3. Menu Détail

- [x] **Pourquoi il ne semble rien faire** : le viewport GPU n'applique ni le
      détail continu ni les matériaux ; seul le rendu CPU les voit. Le détail
      est maintenant évalué dans le maillage GPU, les matières dans le shader,
      et un mode d'aperçu CPU avant/après est disponible.
- [x] **Maillage adaptatif à la forme** (inspiré des couches adaptatives des
      slicers) : subdivision locale selon la courbure / l'erreur géométrique,
      budget de sommets, raccords sans trous.
- [x] **Choix triangles / quads** dans le menu Détail.
- [x] **UV continues** (projection monde) conservées sur le maillage adaptatif,
      export OBJ à quads + GLB triangulé pour Blender.
- [x] Tests : erreur géométrique, étanchéité des raccords, UV, export.

## 4. Menu Matières

- [x] **Pourquoi il ne semble rien faire** : mêmes causes que le détail (GPU
      non branché) ; le rendu CPU ne rafraîchit pas quand le GPU est actif.
- [x] **Presets de roches procédurales** : granite, basalte, calcaire, grès,
      schiste, craie, volcanique — couleur, grain, strates, fissures, patine.
- [x] **Bump mapping** réglable (force, échelle, distance) sur les matériaux
      procéduraux et les textures importées.
- [x] **Displacement mapping** (géométrie) réglable, appliqué au maillage
      d'aperçu et à l'export Blender.
- [x] **Textures personnelles** : import PNG/JPEG, projection monde, bump et
      déplacement lus sur la luminance.
- [x] **Peinture de textures** : pinceau 2D sur une texture de projet, gomme,
      bruit procédural de départ, enregistrement PNG.
- [x] Tests : presets déterministes, bump/displacement, import/peinture.

## 5. Viewport et UI

- [x] **PAN à la souris** (bouton du milieu, clic droit ou `Maj` + glisser),
      en plus de l'orbite et du zoom.
- [x] **Mode AVANT / APRÈS** avec ligne verticale déplaçable à la souris
      (comme le compare de ComfyUI), rendu CPU avant/après. Capture
      `docs/screenshots/compare.png` validée.
- [x] Boutons **Appliquer** / **Revenir** pour valider ou annuler les
      modifications du panneau courant (l'aperçu reste en direct).
- [ ] Tests headless : pan, position de la ligne, application/annulation,
      clavier.

## 6. Validation et livraison

- [x] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` — **264 tests verts**.
- [x] Capture UI avant/après (`docs/screenshots/compare.png`) ; les captures
      atelier Matières/Détail à jour sont dans `/tmp/vfcap8/`.
- [x] Mise à jour `ETAT_ET_REPRISE.md`, `README`/docs utilisateur.
- [~] Binaires `dist/linux-x86_64/` et `SHA256SUMS`.
- [ ] Commit Git (le travail n'est pas poussé sans validation).

## Journal d'avancement

| Date/heure | Étape | Résultat |
|---|---|---|
| 2026-11-19 20:30 | Audit initial | 233 tests de base verts ; causes identifiées pour Détail/Matières (GPU non branché) ; incohérence d'orientation du heightmap Blender confirmée. |
| 2026-11-19 21:00 | Heightmap | Export Blender remis nord-en-haut, blanc = haut ; tests `vf-io` et `vf-export`. |
| 2026-11-19 21:10 | Animation | `Aucune cible`, avance exacte sur courbe, altitude AGL + amortissements caméra/cible, libellés « caméra ». 32 tests `vf-flight`. |
| 2026-11-19 21:25 | Détail | Maillage adaptatif à la forme, raccords 2:1 étanches, UV monde, OBJ quads/n-gons + GLB, panneau UI. |
| 2026-11-19 21:40 | Matières | Presets procéduraux, bump, déplacement, texture personnelle, pinceau 2D, export Blender. |
| 2026-11-19 21:45 | Viewport/UI | Pan, vue CPU, avant/après avec ligne déplaçable, Appliquer/Revenir, détail et matières dans le shader GPU. |
| 2026-11-19 22:35 | Validation visuelle | Capture `workspace_compare.png` : avant clair / après basalt plus sombre, ligne et boutons fonctionnels. |
