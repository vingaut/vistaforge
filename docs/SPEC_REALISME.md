# Spécification — chantier « réalisme géométrique, contrôle et Blender »

> Rédigé le **14 novembre 2026** après audit du dépôt (commit `fb72ad3`,
> 110 tests verts). Ce document reformule la demande, fixe les décisions
> d'architecture et donne une estimation de faisabilité par lot. Il sert de
> contrat de travail pour la session et de point de reprise.

---

## 0. Résumé exécutif

La demande porte sur neuf lots. Trois sont des **changements de modèle**
(subdivision fractale, caméra/rig, mondes tuilés), trois sont des **outils**
(éditeur de heightmap, bridge Blender, UV/remesh), trois sont des **réglages**
(micro-détail par roche, comportements de caméra, modes de rendu).

Décisions structurantes prises dans ce document :

1. **Le heightfield reste la source de vérité.** On ajoute par-dessus un
   **champ de détail continu**, évalué à la résolution du maillage de rendu ou
   d'export. C'est ce qui permet de subdiviser sans ré-échantillonner la carte
   et de garder un détail net à 10 cm comme à 10 km.
2. **Le maillage de rendu devient adaptatif** (quadtree restreint, erreur
   écran, raccords sans fissure) au lieu d'une grille pleine résolution.
   La subdivision ajoute du **déplacement fractal**, elle ne lisse pas.
3. **Les surplombs sont hybrides** : le heightfield gère le socle, une couche
   de **corniches/encorbellements maillée** gère les vraies contre-dépouilles
   locales. Un heightfield pur ne peut pas produire un surplomb global ; on
   l'assume et on livre le cas utile (corniches, lèvres de strates, abris).
4. **Le photoréalisme passe par Blender/Cycles**, pas par un path tracer
   maison ni par DLSS. Le bridge exporte un GLB fidèle, des textures cuites,
   les caméras, le soleil et un script d'import qui règle le rendu Cycles.
   Les GPU présents (Quadro RTX) serviront via OptiX/CUDA dans Blender.
5. **Les grands mondes se font par tuilage**, pas par un heightfield unique de
   32 768². Le mode « immense » accepte 16 384² (1 Go de f32) avec
   avertissement ; au-delà, on passe en tuiles avec variation et raccords.
6. **Pas de DLSS, pas de CUDA maison.** DLSS est un upscaler temps réel lié à
   un moteur de rendu temps réel ; il n'apporte rien à un rendu offline déjà
   supersamplé. Le chemin GPU réaliste est Blender/Cycles + dénoiseur OptiX.

Estimation globale : **49 à 70 jours-homme** pour l'ensemble au niveau
« production crédible ». Cette session vise les lots 1 à 3 et un bridge
Blender v1, puis les lots 4 à 6 par tranches verticales.

---

## 1. Base vérifiée avant de commencer

| Élément | État constaté |
|---|---|
| Workspace | 13 crates, Rust 2024, `cargo test --workspace` : **110 tests verts** |
| Terrain | heightfield f32 régulier, 2,5D, plafond actuel **8192²** |
| Rendu | rasteriseur CPU tuilé, mesh grille pleine résolution, z-buffer, ombres, AO, ciel, eau |
| Détail | un seul `detail_m` + `detail_scale_m` par preset, appliqué après erosion, sans maillage adaptatif |
| Géologie | 9 presets, strates/joints/karst/gullies/spires, schema projet **v5** |
| Modificateurs | 21 opérateurs, pile live, CLI/UI/MCP |
| Caméra | look-at, FOV, near/far ; **pas de DOF, pas de roll, pas de rig** |
| Vol | 6 presets, 4 trajectoires générées, dynamique FPV/hélico ; **pas d'édition graphique de courbe** |
| UI | egui, 10 espaces ; pas d'éditeur de heightmap, pas de graphe de trajectoire |
| Export | PNG, DEM, ASCII ; **pas de glTF, pas de bridge Blender, pas d'UV atlas** |
| Machine | 64 cœurs, 750 Go RAM, 3 GPU Quadro RTX, **Blender 5.3 installé** |

Conséquence : les briques de rendu, d'érosion et de géologie sont saines ; le
chantier est surtout un chantier de **représentation géométrique**, d'**outils
d'édition** et de **passerelle**.

---

## 2. Reformulation des demandes, lot par lot

### Lot 1 — Subdivision fractale et contrôle du détail géométrique

**Demande d'origine.** Pouvoir choisir de subdiviser davantage pour gagner en
précision, voir moins les faces sans obtenir un effet de lissage excessif ; la
géométrie doit être « fractale » ; en s'approchant d'une roche, on doit
percevoir des détails insensés.

**Reformulation opérationnelle.**

- Un réglage **Subdivision** avec trois modes : `auto` (erreur écran),
  `fixe` (niveau 0..8) et `budget` (nombre max de triangles).
- Un **maillage adaptatif** : quadtree restreint (équilibre 2:1), erreur
  projetée à l'écran, raccords par sommets de bord partagés (pas de fissures),
  LOD par distance et par taille de pixel.
- Un **champ de détail continu** `D(x,y)` évalué à la position exacte du
  sommet, composé de bruit fBm/ridged/warpé et des couches du lot 2.
  Le déplacement est ajouté à la hauteur de base, jamais interpolé lissé.
- **Filtrage anti-crénelage** : les octaves dont la longueur d'onde est sous
  l'empreinte du pixel sont atténuées (band-limiting), pas supprimées par un
  flou global. C'est le point qui évite à la fois le scintillement et
  l'aspect « savonné ».
- **Inspecteur de détail** : caméra de contrôle à 1–50 m d'une roche, curseurs
  de fréquence/amplitude/dimension fractale, affichage du nombre de triangles
  et du temps de construction.

**Pourquoi ce n'est pas du lissage.** Un schéma de subdivision classique
(Catmull-Clark) lisse. Ici, chaque nouveau sommet reçoit un déplacement
fractal indépendant, corrélé à l'échelle ; la silhouette gagne des facettes
plus petites, pas une courbe plus douce.

**Faisabilité : élevée (90 %).** Le rasteriseur accepte déjà des triangles
arbitraires ; il faut construire le maillage, le tester et l'intégrer aux
ombres/AO/export. Les 10 % restants sont le polish LOD (transitions, cache,
budget mémoire sur 16k²).

**Effort :** 6–8 jours. **Risque principal :** coût de construction du maillage
et fissures aux transitions ; mitigation par quadtree restreint + tests de
raccord.

---

### Lot 2 — Micro-détails par nature de roche (sillons, arêtes, cratères, surplombs)

**Demande d'origine.** Ajouter des modificateurs de micro-détails, régler les
sillons, les arêtes, les cratères, les surplombs, etc. ; chaque nature de roche
doit avoir sa variation précise pour un réalisme maximal.

**Reformulation opérationnelle.**

Un profil de détail par lithologie, avec des signatures physiques :

| Roche | Signature géométrique | Paramètres exposés |
|---|---|---|
| granite | exfoliation en plaques, joints arrondis, alvéoles | échelle de plaque, bombement, densité d'alvéoles |
| basalte | colonnes, prismes, vésicules, gradins de coulée | espacement, angle, profondeur de vésicule |
| calcaire | bancs, cannelures de dissolution, dolines, lapiaz | épaisseur de banc, densité/rayon de doline, flûtes |
| grès | litage entrecroisé, alvéoles, corniches arrondies | angle de litage, relief, rondeur |
| schiste | foliation, écailles, crêtes acérées | direction, anisotropie, finesse |
| craie | vallons secs, bancs de silex, pentes douces | douceur, fréquence, contraste de dureté |
| volcanique | cordes de lave, scories, petits cratères | longueur d'onde, densité de cratères |
| canyon | strates horizontales, alcôves, cannelures verticales | épaisseur, creux, évasement |
| patagonie | tours verticales, poli glaciaire, exfoliation | verticalité, poli, joints |

Nouveaux opérateurs (pile de modificateurs, en plus des 21 existants) :

1. `grooves` — sillons directionnels, anisotropes, modulés par la pente et la
   dureté des bancs.
2. `ridges` — arêtes saillantes (ridged multifractal), avec exposant de
   netteté et crête « sharp » ou « rounded ».
3. `crater_field` — distribution de cratères (loi de puissance), rebord,
   éjecta, fond plat ou en bol.
4. `exfoliate` — plaques d'exfoliation, détachement par courbure.
5. `vesicles` — alvéoles/puits, densité et profondeur, réservés aux roches
   volcaniques.
6. `undercut` — encorbellement : calcule un masque de contre-dépouille et
   génère la **couche maillée de corniche** (lot 2b).

Tous sont **conscients de la résolution** (distances en mètres, largeur
minimale de 2 cellules, comme les opérateurs existants), **masqués** par
pente/courbure/strate et **déterministes** (seed dérivée).

**Surplombs — décision.** Un heightfield ne peut pas représenter une
contre-dépouille globale. On livre :
- des **corniches** et **lèvres de strates** maillées, locales, sous forme
  d'objets `overhang` rendus et exportés ;
- des **abris/arches** plus tard, par le même mécanisme ;
- pas de surplomb global gratuit sur toute la carte.

**Faisabilité : élevée (85 %), surplombs vrais 50 %.** Les six opérateurs
heightfield sont dans la continuité de l'existant. La couche maillée est un
nouveau petit pipeline mais reste locale.

**Effort :** 5–7 jours. **Risque :** les surplombs maillés doivent se raccorder
au terrain sans flottement ; mitigation par ancrage sur la crête de strate et
tests de non-régression visuelle.

---

### Lot 3 — Éditeur manuel de heightmap

**Demande d'origine.** Un outil d'édition manuelle du heightmap avec des
pinceaux configurables, des formes géométriques, des fractales, etc. ; régler
la taille, la rugosité, l'intensité, l'aliasing, etc.

**Reformulation opérationnelle.**

- **Moteur de pinceau** indépendant de l'UI (`vf-terrain::brush`) :
  - taille en **mètres** (pas en pixels), forme, dureté, courbe de retombée ;
  - **rugosité** : perturbation du noyau par bruit (domain warp) ;
  - **intensité** : dose par seconde ou par échantillon de trait ;
  - **aliasing** : couverture analytique des formes + supersampling du noyau
    fractal, jamais un simple flou ;
  - espacement, jitter, angle, aspect, symétrie X/Y/radiale ;
  - pression absente (pas de tablette) mais vitesse de glissement module
    l'intensité si activé.
- **Modes** : élever, abaisser, lisser, aplatir, terrasse, crête, sillon,
  cratère, bruit, warp, clone, peindre un masque (neige/végétation/roche).
- **Formes** : disque, carré, losange, anneau, étoile, polygone, ligne,
  rectangle, ellipse, main levée ; **formes fractales** : bruit seuil,
  ridged, cratère, Voronoï.
- **UI** : vue de dessus avec hillshade, zoom/pan, curseur de pinceau à
  l'échelle, aperçu du trait, symétrie, **undo/redo par tuiles** (diff de
  région, pas une copie de 67 Mo), presets de pinceaux.
- **Cohérence** : le pinceau édite le heightfield de base ; le champ de détail
  reste procedural par-dessus. On peut aussi peindre un masque d'amplitude de
  détail (option).

**Faisabilité : élevée (90 %).** C'est de l'algorithmique locale et de l'UI
egui ; le point délicat est l'undo performant et le rendu de l'aperçu sans
recalcul complet.

**Effort :** 4–6 jours. **Risque :** UI non testable sur cette machine sans
écran (compilation seulement) ; mitigation par tests du moteur et capture
headless plus tard.

---

### Lot 4 — Animation : édition graphique du chemin et rig type LightWave

**Demande d'origine.** Éditer le path graphiquement, disposer d'outils de
poursuite du point de vue ; caméra comme dans LightWave : un point de vue et
une caméra liée, réglable en FOV, profondeur de champ ; tracer la courbe du
point de vue et régler le mouvement de poursuite de la caméra.

**Reformulation opérationnelle.**

Modèle `CameraRig` à trois objets :

1. **Eye / point de vue** : courbe 3D (clé + poignées Bézier/Hermite), temps
   par clé, tension/continuité/biais, boucle ouverte/fermée, longueur d'arc.
2. **Target / point de poursuite** : point fixe, courbe propre, ou suivi de
   l'eye avec avance (look-ahead) et décalage ; c'est le « null » de
   LightWave.
3. **Camera / optique** : liée à l'eye (rigide, ressort, ou libre), FOV,
   capteur, **profondeur de champ** (distance de mise au point, ouverture,
   bokeh), roll explicite, up vector.

Éditeur graphique :
- vue de dessus (hillshade) avec poignées Bézier, édition des points et des
  tangentes, insertion/suppression, snap au sol, affichage des temps ;
- timeline avec curseur, lecture/arrêt, pas-à-pas ;
- graphes de vitesse et d'accélération par axe ;
- outil « poursuite » : on dessine la cible, on règle l'avance, le retard, le
  damping, le cadrage ;
- aperçu 3D basse résolution à un temps donné (déjà amorcé par le panneau
  Vol).

**Faisabilité : élevée (80 %).** La simulation et la courbe sont de la
géométrie déterministe ; l'éditeur graphique est le gros du travail. La
parité complète avec LightWave (scène, IK, expressions) n'est pas visée.

**Effort :** 8–10 jours. **Risque :** ergonomie de l'éditeur ; mitigation par
un mode « canvas 2D + timeline » simple avant tout ajout 3D.

---

### Lot 5 — Comportements de caméra, presets d'accélération, altitude et parois

**Demande d'origine.** Roulis dans les virages ; plusieurs pré-réglages pour
l'accélération de chaque axe (drone FPV, oiseau, hélicoptère, ballon
dirigeable, avion de chasse, manuel) ; maintien automatique de la caméra et/ou
du point de vue à une hauteur du sol de référence fixable et/ou à une distance
des parois ; mode automatique avec courbe Bézier, accélérations latérales et
longitudinales, vecteur de poursuite, roulis et évitement de paroi.

**Reformulation opérationnelle.**

- **Presets de dynamique** avec limites par axe (avant/arrière, latéral,
  vertical), jerk, traînée, gravité, vitesse max, gain de roulis, réponse :

| Preset | Accél. avant | Accél. latérale | Accél. verticale | Roulis max | Signature |
|---|---:|---:|---:|---:|---|
| Drone FPV | 25 m/s² | 30 m/s² | 20 m/s² | 75° | nerveux, jerk élevé |
| Oiseau | 8 | 12 | 10 | 55° | battement/plané, inertie moyenne |
| Hélicoptère | 4 | 6 | 5 | 25° | lourd, nacelle stable |
| Ballon dirigeable | 1,5 | 1,5 | 1 | 8° | très lent, dérive |
| Avion de chasse | 40 | 60 | 30 | 85° | très haute énergie |
| Manuel | libre | libre | libre | libre | courbe brute |

- **Roulis** : `bank = atan(v²·κ/g) · gain`, borné, lissé, plus une courbe de
  roll explicite additive ; compensation de lacet.
- **Contraintes** : hauteur au-dessus d'un **sol de référence** (plan fixe +
  terrain), distance aux **parois** (champ de distance 2D vers les cellules
  raides), appliquées indépendamment à l'eye, à la caméra et/ou à la target,
  avec seuil, douceur, anticipation.
- **Évitement** : force latérale proportionnelle au dépassement du seuil,
  dirigée par le gradient du champ de distance, amortie ; pas de téléportation.
- **Mode automatique** : on dessine la courbe de l'eye, on règle les
  accélérations latérale/longitudinale, la caméra poursuit avec avance et
  roulis, et l'évitement corrige la trajectoire si une paroi approche.

**Faisabilité : élevée (85 %).** Le champ de distance, le suiveur jerk-limité
et le roulis sont classiques. Le mode automatique est une boucle de contrôle
simple, pas un planificateur global ; il peut être localement myope.

**Effort :** 4–6 jours. **Risque :** oscillations de l'évitement ; mitigation
par amortissement critique et tests de scénarios canyon.

---

### Lot 6 — Grands paysages : taille immense, tuilage, seamless

**Demande d'origine.** Au démarrage, choisir une taille immense (sans limite
autre que la RAM, avec un défaut raisonnable) ; ou un tuilage avec instances
du décor, répétitions variées pour éviter la symétrie ; option « seamless »
avec raccords invisibles, ordonnée ou désordonnée.

**Reformulation opérationnelle.**

Trois modes dans `WorldSettings` :

1. **Single** : 1024² à 16 384². Défaut **2048²**. Avertissement mémoire et
   estimation avant allocation. Plafond dur 16 384² (1 Go de f32, hors
   masque) ; au-delà, refus explicite avec renvoi vers le mode tuilé.
2. **Tiled** : N×M instances de la tuile source, avec les 8 symétries
   (rotations 90°, miroirs), échelle, décalage de seed de détail et variation
   de matériau par instance. Répétition non symétrique par affectation
   pseudo-aléatoire déterministe. Rendu avec culling et LOD par tuile ;
   export en collection d'instances dans Blender.
3. **Seamless** : rendre la tuile source **périodique** :
   - correction de bord par blending périodique puis **résolution de Poisson**
     pour annuler exactement le saut de hauteur et de pente aux jonctions ;
   - jeu de tuiles de Wang (v1 : 4 variantes, bords compatibles) pour un
     assemblage ordonné ou désordonné sans couture visible ;
   - les masques de matériaux dérivent de la hauteur et des strates, donc
     suivent automatiquement.

**Décision importante.** On ne promet pas un heightfield unique de 32 768²
dans cette session : le rendu, l'édition et l'undo deviendraient un projet
out-of-core à part entière. Le mode tuilé couvre le besoin « immense » avec un
budget mémoire borné.

**Faisabilité : moyenne (60 %).** Tuilage : 90 %. Seamless crédible : 60 %
(correction de bord + blending ; le désordonné parfait est plus dur). 32k
monolithique : 20 %.

**Effort :** 8–12 jours. **Risque :** répétition visible et coutures ; tests
automatiques de discontinuité aux bords.

---

### Lot 7 — Modes de rendu et GPU

**Demande d'origine.** Choisir un mode rapide, soigné ou photoréaliste, avec
les meilleures techniques accessibles ; utiliser les GPU (CUDA, DLSS).

**Reformulation opérationnelle.**

- `RenderMode::Fast` : aperçu actuel (pas d'ombres/AO, SSAA 1).
- `RenderMode::Refined` : ombres, AO multi-échelle, SSAA 2, PCF, eau, nuages.
- `RenderMode::Photoreal` : SSAA 3, ombres 8192, AO, **DOF** (bokeh en
  post), bloom, grain fin, tone mapping ACES ; ou **moteur Blender/Cycles**
  avec OptiX et dénoiseur.
- Nouveau `--engine cpu|blender` : `blender` écrit le GLB + textures, lance
  Blender headless avec Cycles, récupère le PNG. C'est le chemin GPU réel.
- **DLSS : non.** Explication : upscaler temps réel, dépendant d'un moteur
  temps réel, sans intérêt pour un rendu offline supersamplé. **CUDA maison :
  non.** Cycles/OptiX fait mieux et est déjà installé.
- Si un jour un viewport temps réel est ajouté, `wgpu` + FSR/DLSS pourra être
  étudié ; ce n'est pas dans ce chantier.

**Faisabilité : élevée (85 %) pour les modes CPU ; 90 % pour le chemin
Blender GPU ; 5 % pour DLSS.** **Effort :** 3–4 jours. **Risque :** le DOF
CPU coûte cher ; mitigation par post-traitement séparable et aperçu rapide.

---

### Lot 8 — Bridge Blender fidèle

**Demande d'origine.** Travailler le bridge avec Blender pour que la scène
soit exportable très fidèlement et qu'on puisse faire le texturing dans
Blender ; la scène importée doit permettre facilement des textures multiples
fidèles.

**Reformulation opérationnelle.**

- Nouveau crate `vf-export` :
  - **GLB 2.0** écrit à la main (JSON + buffer binaire), sans dépendance
    lourde : positions, normales, tangentes, UV0 (plan) et UV1 (atlas),
    couleurs de sommets (roche/neige/éboulis/végétation/humidité/strate),
    indices de matériaux, caméra, soleil directionnel, plan d'eau, sol.
  - **Sidecar JSON** : palette, soleil, caméra, unités, CRS, seed, version,
    paramètres de roche, chemin du rig, réglages de rendu.
  - **Textures cuites** par matériau : albédo, rugosité, normale, AO, hauteur
    (PNG 16 bits), masques. Résolution 1k/2k/4k/8k.
  - **UV** : planaire pour le terrain, atlas par projection de boîte pour les
    falaises/surplombs (lot 9) ; sinon le script Blender fait un Smart UV.
  - **Remesh** : LOD/decimation pour les objets lourds ; le script Blender
    propose voxel remesh / QuadriFlow en option.
- Script `bridge/blender/vistaforge_import.py` :
  - importe le GLB, règle les unités et la colorimétrie (AgX/Filmic) ;
  - recrée les matériaux PBR par lithologie à partir des textures cuites et
    des masques ; nœuds de mélange roche/neige/éboulis/végétation ;
  - ajoute le soleil, le monde, la caméra (FOV, DOF), l'eau ;
  - option displacement (texture de hauteur + subdivision) ;
  - option remesh + Smart UV + UDIM ;
  - option Cycles/OptiX (échantillons, dénoiseur, résolution) et rendu.
- Commande `vistaforge blender export <projet> <dossier>` et
  `vistaforge blender render <projet> <image>` (appel headless).
- **Validation** : un projet de test exporté, importé dans Blender 5.3
  headless, rendu Cycles, image comparée à la référence CPU.

**Faisabilité : élevée (90 %).** Le format GLB est simple ; le travail réel
est la cuisson des textures et le script Blender. Blender est installé, donc
la validation de bout en bout est possible sur cette machine.

**Effort :** 5–7 jours. **Risque :** fidélité des matériaux entre le shader
CPU et les nœuds Blender ; mitigation par textures cuites (on exporte le
résultat, pas la recette) et comparaison d'images.

---

### Lot 9 — UV map et remaillage dans VistaForge

**Demande d'origine.** Si l'UV map et le remaillage sont faisables dans
VistaForge, ajouter cette possibilité ; l'idée est que la scène importée sous
Blender permette facilement des textures multiples fidèles.

**Reformulation opérationnelle.**

- **UV** :
  - terrain : projection planaire (une île de charte) ;
  - falaises/surplombs : segmentation par normale (6 directions) + croissance
    de régions, projection par charte, **packing** (shelf/skyline), densité
    de texels égalisée, duplication des sommets aux coutures ;
  - export UV0 (plan) et UV1 (atlas) ; option « Blender Smart UV » si l'atlas
    n'est pas suffisant.
- **Remaillage** :
  - le maillage adaptatif est déjà un remesh quadratique du heightfield ;
  - décimation quadrique pour les objets lourds ;
  - simplification par LOD ; conservation des UV autant que possible ;
  - **pas d'auto-retopologie de production** (QuadriFlow-like) dans
    VistaForge : le script Blender l'expose en option.
- **Textures multiples fidèles** : matériaux par lithologie + masques cuits +
  UV atlas ; chaque zone peut recevoir sa texture dans Blender.

**Faisabilité : moyenne (65 %).** UV atlas : 75 %. Décimation : 80 %.
Auto-retopo : 25 %. **Effort :** 4–6 jours. **Risque :** chartes qui se
chevauchent et coutures visibles ; mitigation par tests de recouvrement et
mode planaire par défaut.

---

## 3. Décisions d'architecture (ADR)

### ADR-1 — Heightfield canonique + champ de détail continu

Le `HeightField` reste la donnée editable, masquable, érodable et exportable
en DEM. Le détail haute fréquence n'est **pas** cuit dans la carte : il est
évalué par une fonction `SurfaceSampler` au moment du rendu et de l'export.
Avantages : pas de ré-échantillonnage, détail indépendant de la résolution,
UNDO léger, compatibilité DEM. Inconvénient : les modificateurs qui ont
besoin du détail (cratères, érosion fine) doivent soit cuire, soit passer par
le champ de détail.

### ADR-2 — Maillage adaptatif, pas de subdivision lissante

Quadtree restreint, erreur écran, 2:1, sommets de bord partagés. Chaque sommet
évalue le champ de détail. On peut choisir un niveau fixe pour l'export
(maillage régulier) ou adaptatif pour le rendu. Pas de Catmull-Clark.

### ADR-3 — Surplombs hybrides

Le socle reste 2,5D. Les contre-dépouilles sont des objets maillés locaux
(`overhang`), ancrés sur les strates, rendus avec le même matériau, exportés
comme objets séparés. On ne promet pas de surplomb global.

### ADR-4 — Mondes tuilés, pas de heightfield 32k monolithique

`Single` jusqu'à 16 384², `Tiled` et `Seamless` au-delà. Le tuilage est un
mode de composition (rendu/export), pas une duplication de la donnée.

### ADR-5 — Blender est le chemin photoréaliste et GPU

Le rendu CPU reste le rendu de contrôle déterministe. Le photoréalisme et le
GPU passent par Cycles/OptiX via le bridge. Pas de DLSS, pas de CUDA maison.

### ADR-6 — Schéma projet v6, migration douce

Ajout de `detail`, `rig`, `world`, `export`. `flight` reste pour la
compatibilité et devient un cas particulier du rig. Les champs manquants
prennent leur défaut ; test de migration v1→v6.

### ADR-7 — Déterminisme

Toute nouvelle génération (détail, cratères, tuilage, évitement) est seedée et
reproductible. Aucun accès horloge/aléatoire système dans le pipeline.

---

## 4. Modèle de données (esquisse, schéma v6)

```rust
// vf-core::detail
pub struct DetailSettings {
    pub enabled: bool,
    pub profile: String,          // "granite", "basalt", ...
    pub scale_m: f32,             // longueur d'onde de base
    pub amplitude_m: f32,
    pub fractal_dimension: f32,   // 2.0..3.0
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    pub warp: f32,
    pub band_limit: bool,         // anti-aliasing par empreinte pixel
    pub grooves: GrooveSettings,
    pub ridges: RidgeSettings,
    pub craters: CraterFieldSettings,
    pub exfoliation: ExfoliationSettings,
    pub vesicles: VesicleSettings,
    pub undercut: UndercutSettings,
    pub mask: DetailMask,         // pente, courbure, altitude, strate
    pub seed: u64,
}

// vf-core::rig
pub struct CameraRig {
    pub enabled: bool,
    pub eye: Path3D,
    pub target: TargetRig,        // Fixe | Path | SuitEye { lead_s, offset }
    pub camera: OpticalRig,       // fov, sensor, dof, roll, link
    pub body: MotionProfile,      // preset + limites par axe + jerk
    pub constraints: Constraints, // agl, reference_z, wall_distance, qui
}

// vf-core::world
pub struct WorldSettings {
    pub mode: WorldMode,          // Single | Tiled | Seamless
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub variation: TileVariation, // rotations, miroirs, seed, échelle
    pub seam: SeamSettings,       // none | blend | poisson | wang
}

// vf-core::export
pub struct ExportSettings {
    pub format: ExportFormat,     // Glb | Gltf | Blend
    pub texture_size: u32,        // 1024..8192
    pub uv_mode: UvMode,          // Planar | Atlas | BlenderSmart
    pub remesh: RemeshMode,       // None | Decimate | BlenderVoxel
    pub include_water: bool,
    pub include_rig: bool,
    pub bake_maps: bool,
}
```

---

## 5. Plan de la session

Ordre choisi pour maximiser la valeur et valider tôt la chaîne complète :

1. **P0 — Socle détail + maillage adaptatif** (`vf-detail`, `vf-mesh`,
   intégration renderer). Démo gros plan roche + plan large sans facettes.
2. **P1 — Micro-opérateurs + presets roches** (6 opérateurs, profils,
   corniches maillées). Démo par lithologie.
3. **P2 — Bridge Blender v1** (GLB, textures, script, rendu headless).
   Validation de bout en bout sur un projet de test.
4. **P3 — Éditeur manuel** (moteur de pinceau, UI Sculpter, undo).
5. **P4 — Rig caméra + éditeur graphique** (Bézier, presets, roulis,
   contraintes, évitement).
6. **P5 — Mondes** (tailles, tuilage, seamless v1) et **modes de rendu**.
7. **P6 — UV/remesh, docs, reprise**.

Critères de sortie à chaque lot :

- `cargo test --workspace` vert, `cargo clippy --workspace --all-targets --
  -D warnings` propre ;
- au moins un test unitaire du nouveau comportement et un test de
  non-régression déterministe ;
- une image de démonstration dans `demo-out/` ;
- documentation mise à jour (`docs/`, README, `ETAT_ET_REPRISE.md`).

---

## 6. Limites assumées (non-objectifs)

| Sujet | Décision |
|---|---|
| Surplomb global sur heightfield | non ; corniches/arches maillées locales |
| Heightfield 32 768² monolithique | non ; mode tuilé |
| DLSS | non ; sans objet pour le rendu offline |
| Path tracer CUDA maison | non ; Blender Cycles/OptiX |
| Auto-retopologie de production | non ; décimation + remesh Blender |
| Parité complète LightWave (IK, expressions, scène) | non ; rig eye/target/camera |
| Textures scannées PBR | non ; procédural cuit + texturing Blender |
| Round-trip Blender → VistaForge | non ; export aller seulement |

---

## 7. Risques et parades

| Risque | Impact | Parade |
|---|---|---|
| Maillage adaptatif qui fissure | trous visibles | quadtree restreint + tests de bord |
| Détail qui scintille | bruit temporel | band-limiting + SSAA |
| Surplombs flottants | artefacts | ancrage strate + test de contact |
| Undo trop gourmand | OOM sur 16k² | diffs par tuiles |
| Éditeur graphique confus | perte de temps | canvas 2D simple d'abord |
| Seamless imparfait | coutures | Poisson + blending + tests de discontinuité |
| Bridge Blender non fidèle | perte de confiance | textures cuites, comparaison d'images |
| UI non testable sans écran | régressions | tests moteur + captures headless |

---

## 8. Notes techniques et références

- Subdivision adaptative : restricted quadtree / RTIN, erreur écran, 2:1
  balancing, edge stitching.
- Détail fractal : fBm, ridged multifractal, domain warp, dimension fractale
  `D = 2 + log(gain)/log(lacunarity)` (approximation), band-limiting par
  footprint.
- Surplombs : corniche = lèvre maillée le long d'une strate dure, ancrée sur
  la crête ; extrusion avec profil de rayon et bruit.
- Brush : couverture analytique (SDF de la forme), supersampling, domain warp
  pour la rugosité ; undo par blocs 128².
- Rig : courbe de Bézier par morceaux, table de longueur d'arc, filtre
  jerk-limité (S-curve), ressort critique pour l'évitement.
- Champ de distance aux parois : distance euclidienne approchée (chamfer 3-4
  ou EDT) sur les cellules de pente > 55°.
- Seamless : blending périodique + Poisson `Δu = f` avec contraintes de bord,
  jeu de tuiles de Wang.
- Blender : glTF 2.0, Principled BSDF, AgX, Cycles OptiX, dénoiseur OpenImageDenoise.
- Rendu : tone mapping ACES, PCF, HBAO, bokeh (scatter-as-gather), bloom.

---

*Fin de la spécification. Les estimations seront revues à la fin de chaque
lot ; le journal de session et l'état réel seront consignés dans
`docs/ETAT_ET_REPRISE.md`.*
