# Plan actif — sessions des 19 et 20 novembre 2026

> **Session en cours (20 novembre)** : voir
> [`SESSION_2026-11-20.md`](SESSION_2026-11-20.md). Le présent fichier garde
> l'historique de la session du 19 et le journal cumulé.

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
      et une vue CPU complète est disponible.
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
- [x] **Mode AVANT / APRÈS** livré le 19, puis **retiré le 20 novembre** sur
      demande : il ajoutait de la confusion à côté des vues GPU/CPU.
- [x] Boutons **Appliquer** / **Revenir** retirés avec le comparateur.
- [ ] Tests headless : pan, bascule GPU/CPU, clavier.

## 5 bis. Corrections de la deuxième passe (19 novembre, soir)

- [x] **Caméra conservée** après génération / import / composition
      (`Conserver la caméra`, bouton `Recadrer`) et après **Annuler/Rétablir** :
      l'angle, la distance et le zoom de la vue d'inspection ne sont plus perdus.
- [x] **Maillage adaptatif par angle de face** : le niveau suit désormais
      l'écart au plan local (courbure / arête vive), pas la hauteur brute.
      Une pente raide mais lisse reste grossière ; une crête pointue est
      subdivisée. Test `a_planar_slope_is_not_over_subdivided`.
- [x] **Deuxième passe après bump/displacement** : `MaterialSampler` fait
      évaluer le déplacement de matière par le constructeur adaptatif
      (`SurfaceSampler`). Tolérance resserrée et subdivision minimale ×2 quand
      un déplacement est actif.
- [x] **Adaptatif dans le viewport GPU** : le maillage GPU suit la même
      recette (SurfaceSampler, tolérance, subdivision) que le CPU.
- [x] **Déplacement vertical** (et non le long de la normale) : fini les
      aiguilles triangulaires sur les pentes raides.
- [x] **Empreinte constante** sur tout le maillage adaptatif : les sommets
      partagés entre blocs ont exactement la même hauteur, donc plus de trous
      ni de triangles noirs. Test avec détail actif.
- [x] **Qualité d'aperçu** : sélecteur Brouillon / Normal / Haute / Finale,
      déplacé le 20 novembre dans la barre d'outils unique.
- [x] **Comparateur** retiré le 20 novembre (interface simplifiée).
- [x] **Sélecteur de fichier** pour la texture personnelle (PNG/JPG/TIFF).

## 6. Validation et livraison

- [x] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` — **272 tests verts**.
- [x] Captures Matières/Détail à jour dans `docs/screenshots/` (le
      comparateur a été retiré).
- [x] Mise à jour `ETAT_ET_REPRISE.md`, `README`/docs utilisateur.
- [x] Binaires `dist/linux-x86_64/` et `SHA256SUMS`.
- [~] Commit Git (le travail n'est pas poussé sans validation).

## 7. Session du 20 novembre — simplification et relief procédural

- [x] Suppression complète du mode Avant/Après (UI, état, dessin, captures).
- [x] Vue unifiée **GPU / CPU** + un seul **Aperçu** + qualité dans la barre
      d'outils ; suppression des rangées et boutons de preview répétés.
- [x] Bouton **Réinitialiser l'animation**.
- [x] Détail adaptatif : tolérance jusqu'à **0,001 m**, persistance des réglages
      lors d'un changement de profil, bouton **Défaut maillage** et
      **Réinitialiser le détail**.
- [x] Bruit de relief nul par défaut (`detail_normal = 0`, `bump = 0`).
- [x] Résolutions rapides **2048 / 4096 / 8192**.
- [x] Nouvelles couches de **géométrie** : tafoni/nids d'abeille, bancs
      différentiels, joints, oolithes.
- [x] Nouveaux presets roche/détail : chert, andésite, marbre, phyllite,
      ardoise, gneiss, conglomérat, travertin.
- [x] Lien optionnel matériau ↔ géométrie (le profil de détail suit la roche).
- [x] Optimisations : budget adaptatif par buckets, échantillonnage 5×5,
      bake de textures sur la surface de base.
- [x] Validation visuelle : rendu propre par défaut, bancs de grès et
      géométrie adaptative contrôlés sur falaise.
- [x] Docs/help et fichier de reprise à jour ; **272 tests**, commit, binaires.

## Journal d'avancement

| Date/heure | Étape | Résultat |
|---|---|---|
| 2026-11-19 20:30 | Audit initial | 233 tests de base verts ; causes identifiées pour Détail/Matières (GPU non branché) ; incohérence d'orientation du heightmap Blender confirmée. |
| 2026-11-19 21:00 | Heightmap | Export Blender remis nord-en-haut, blanc = haut ; tests `vf-io` et `vf-export`. |
| 2026-11-19 21:10 | Animation | `Aucune cible`, avance exacte sur courbe, altitude AGL + amortissements caméra/cible, libellés « caméra ». 32 tests `vf-flight`. |
| 2026-11-19 21:25 | Détail | Maillage adaptatif à la forme, raccords 2:1 étanches, UV monde, OBJ quads/n-gons + GLB, panneau UI. |
| 2026-11-19 21:40 | Matières | Presets procéduraux, bump, déplacement, texture personnelle, pinceau 2D, export Blender. |
| 2026-11-19 21:45 | Viewport/UI | Pan, vue CPU, avant/après (retiré le 20), détail et matières dans le shader GPU. |
| 2026-11-19 22:35 | Validation visuelle | Capture `workspace_compare.png` : avant clair / après basalt plus sombre, ligne et boutons fonctionnels. |
| 2026-11-19 23:45 | Deuxième passe | Caméra conservée, adaptatif par courbure, déplacement vertical + seconde passe, qualité d'aperçu, picker texture. 272 tests. Comparateur retiré le 20. |
