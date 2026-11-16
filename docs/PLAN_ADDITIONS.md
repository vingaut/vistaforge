# VistaForge — audit et plan d'extension

> État au 13 novembre 2026. Ce document remplace la lecture « 30 semaines » du
> par un plan court, centré sur trois ajouts différenciants :
> un modèle géologique procédural, un module de survol FPV/hélicoptère, et une
> interface en barres de fonctions (sans graphe nodal).

---

## 1. Audit : où en est réellement le projet

### 1.1 Ce qui existe et fonctionne

Mesures faites sur cette machine (64 cœurs, 750 Go de RAM) :

| Élément | État | Mesure |
|---|---|---|
| Workspace Rust (10 crates + 1 nouvelle) | compile, tests verts | 59 tests + 23 nouveaux |
| Terrain Classic / Modern | fonctionnel, 7 presets | 1026² généré + érodé en ~3,6 s |
| Érosion hydraulique + thermique | fonctionnelle | 150 k gouttes sur 2050² |
| DEM legacy | lecture/écriture validées sur le corpus local | round-trip |
| Rendu CPU déterministe | tuilé, z-buffer, ombres, AO, ciel, eau | 960×540 en **477 ms**, 1920×1080 SSAA2 en **5,6 s** |
| Projet `.vfproj` | ZIP + RON + heightmap f32 | schéma v2 (géologie) |
| CLI | generate / import / dem / scene / render | ~1 100 lignes |
| MCP | 19 outils JSON-RPC | ~1 470 lignes |
| UI egui | viewport + panneaux | ~1 050 lignes |

Le socle est **meilleur que ne le dit le plan** : le déterminisme, le CLI, le
MCP et le format de projet sont déjà là. Le rendu est un rasteriseur CPU
autonome — pas de GPU, pas de dépendance lourde — ce qui est un atout pour le
batch, les tests et la reproductibilité.

### 1.2 Les vrais manques

1. **Aucune animation** : la caméra est un `look-at` statique. Pas de timeline,
   pas de séquence, pas d'export vidéo.
2. **Aucun modèle de matériau** : le rendu mélange 6 couleurs de palette par
   altitude et pente. Pas d'éboulis, pas de neige irrégulière, pas de strates.
3. **Détail de surface absent** : le terrain est lissé par l'érosion, sans
   micro-relief ni normales de détail. C'est la cause principale de l'écart
   avec l'image de référence.
4. **UI monolithique** : un seul panneau avec des `CollapsingHeader`, une
   trentaine de champs codés en dur dans `VistaForgeApp`. Chaque nouveau
   réglage demande de toucher la structure de l'app.
5. **Pas d'undo/redo** à la date de cet audit — **corrigé le 16 novembre 2026** :
   historique global réglable (profondeur, mémoire, regroupement), y compris
   les traits de pinceau. Voir la session du 16 novembre dans `ETAT_ET_REPRISE.md`.
6. **Pas de dépôt Git** : le projet n'est pas versionné (`git` absent), ce qui
   est un risque majeur dès qu'on ajoute des modules.
7. **Le plan et le code ont divergé** : le plan décrit `wgpu`, une timeline et
   un bridge Blender ; le code est un rasteriseur CPU avec egui. Le plan doit
   être réaligné, pas suivi à la lettre.

### 1.3 Ce que dit l'image de référence

L'image fournie (montagne stratifiée, éboulis, névés, crêtes nettes) ne doit
presque rien à une « fonction géologique » supplémentaire. Elle doit sa
qualité à quatre couches :

| Couche | Référence | VistaForge aujourd'hui |
|---|---|---|
| Géométrie | micro-relief métrique, crêtes vives, éboulis | heightfield lissé, pas de détail |
| Matériaux | strates irrégulières, éboulis clairs, neige en plaques, herbe dans les creux | 6 couleurs par altitude/pente |
| Lumière | ombres nettes et profondes, fort contraste solaire | ombre douce, ambiante faible |
| Post | micro-contraste, netteté, brume | tone mapping seul |

**Conclusion :** le chemin vers ce niveau passe d'abord par la **résolution +
le détail + les matériaux**, pas par dix opérateurs géologiques de plus. Le
modèle géologique sert à produire la *structure* (strates, joints, karst) que
les matériaux doivent ensuite lire.

---

## 2. Veille concurrentielle (synthèse)

| Outil | Forces | Faiblesses | Prix indicatif |
|---|---|---|---|
| **Gaea** | graphe nodal, érosion de référence, tuiles 256K, ponts UE/Houdini | graphe dense, Windows, VRAM | 99–299 $ |
| **World Creator** | GPU, biomes, scattering, path tracing, MCP | GPU imposé, v4 immature | 149 $ |
| **World Machine** | érosion de référence, CPU, VDM 3D en 2026 | pas de rendu final, UI datée | 119–299 $ |
| **Terragen** | path tracer CPU, atmosphères, animation caméra | lent, authoring limité | 199–1020 $ |
| **Instant Terra** | CPU nodal très rapide, déterministe, API | peu de rendu | 149–199 $ |
| **Vue** | végétation, atmosphères | développement arrêté (2024) | gratuit/open source |
| **Blender + addons** | flexibilité, scattering | jamais clé en main | gratuit + 30–90 € |

**Tendances 2025–2026** : écosystèmes (biomes, scattering) plutôt que textures ;
retour au temps réel (UE/Unity/Godot) ; échelle (planètes, DEM réels) ;
simulations physiques (sable, neige, rivières, surplombs) ; IA assistée via MCP.

**Ce que personne ne fait bien, et qui nous appartient :**

1. **Sémantique géologique** : des *familles de roches* nommées, pas des bruits
   génériques. Un preset « grès de l'Utah » qui règle d'un coup dureté, pendage,
   épaisseur de bancs, joints et couleur.
2. **Animation caméra pro** : Terragen a des chemins de caméra, personne n'a de
   dynamique de drone FPV crédible intégrée à un générateur de terrain.
3. **UX sans graphe** : l'anti-Gaea. Des barres de fonctions, des sliders, des
   valeurs numériques, un empilement de modificateurs lisible.
4. **Reproductibilité** : seed + version + hash de bake, CLI batch, sortie
   bit-à-bit. Déjà notre ADN ; à pousser comme argument.
5. **CPU + headless + MCP** : un agent peut générer, régler et rendre une scène
   sans GPU ni session graphique.

---

## 3. Proposition A — Modèle géologique procédural

### 3.1 Principe

Un **modèle de roche** est un petit jeu de paramètres physiques, pas une pile
de nœuds :

```
famille de roche
 ├─ bancs (épaisseur, pendage, dureté, couleur)
 ├─ fractures (maille, profondeur, orientation)
 ├─ dissolution (dolines, lapiaz)
 └─ détail de surface (amplitude, échelle)
```

Chaque famille est un preset de ces paramètres. L'utilisateur peut partir d'un
preset puis desserrer les sliders ; chaque slider reste nommé, borné, avec une
unité.

### 3.2 Opérateurs (heightfield)

Déjà implémentés dans `crates/vf-geology` :

| Opérateur | Effet | Paramètres exposés |
|---|---|---|
| **Strates** | bancs durs en relief, bancs tendres en creux, pendage | épaisseur 1–120 m, pendage 0–80°, azimut, contraste de dureté 0–1, relief différentiel 0–40 m, déformation 0–2, échelle de déformation, contraste de couleur |
| **Joints orthogonaux** | réseaux de fractures, arêtes vives | espacement 2–200 m, profondeur 0–20 m, largeur, gigue, angle |
| **Colonnes hexagonales** | orgues basaltiques | idem + réseau hexagonal |
| **Dissolution karstique** | dolines, ouvalas, lapiaz | densité 0–1, rayon 5–800 m, profondeur 0–60 m, cannelures 0–1 |
| **Détail de surface** | micro-relief sur les pentes | amplitude 0–20 m, échelle 1–1000 m |

Les opérateurs sont **conscients de la résolution** : une fracture plus étroite
que deux cellules est élargie, une doline plus petite que 2,5 cellules est
agrandie. Sans cela, le même preset devient du bruit à 1026² et du crénelage à
2050².

### 3.3 Presets livrés

`granite`, `basalte`, `calcaire`, `grès`, `schiste`, `craie`, `volcanique`.

Chacun règle les quatre blocs ci-dessus. Exemple de ce que raconte un preset :

| Preset | Signature | Réglages clés |
|---|---|---|
| Grès | strates, corniches, canyons | bancs 26 m, pendage 6°, dureté 0,8, relief 18 m |
| Calcaire | karst, dolines, lapiaz | bancs 12 m, dissolution 0,55, cannelures 0,5 |
| Basalte | orgues, plateaux | colonnes hex 22 m, profondeur 6,5 m |
| Schiste | foliation redressée, crêtes dentelées | bancs 6 m, pendage 60°, détail 2,5 m |
| Granite | tors, boules, joints | pas de strates, joints 45 m, détail 2,2 m |

### 3.4 Couleur

Le rendu réévalue la **même stratigraphie** que la géométrie : chaque banc a
une teinte, une dureté et une dérive latérale. Les bandes de couleur suivent
donc les corniches au lieu d'être plaquées.

### 3.5 Limite assumée : le heightfield

Une altitude par (x, y) interdit surplombs, arches et grottes. Stratégie par
étapes :

1. **Normales de détail + triplanar** (aucune géométrie) — fort en vue
   rapprochée, coût quasi nul.
2. **Opérateurs heightfield** — c'est ce qui est fait ; couvre 95 % des
   silhouettes.
3. **Mesh de falaise** plaqué sur les crêtes les plus raides, plus tard.
4. **SDF/mesh local** pour une arche « héros », à réserver à des cas isolés.

Ne pas voxeliser tout le terrain : le budget CPU ne le permet pas et le gain
visuel est marginal à distance de paysage.

---

## 4. Proposition B — Module de vol FPV / hélicoptère

### 4.1 Pourquoi c'est différenciant

Aucun générateur de terrain grand public n'intègre une dynamique de drone. Les
chemins de caméra existants sont des interpolations de keyframes : ça glisse,
ça ne *vole* pas. Le « feeling » FPV vient de trois choses : l'assiette commande
la vitesse, le taux de rotation est la commande (mode acro), et la caméra est
solidaire du châssis avec une inclinaison fixe.

### 4.2 Modèle

État : position, vitesse, quaternion, vitesse angulaire, poussée filtrée.
Intégration à pas fixe (240 Hz), déterministe.

```
ṗ = v
v̇ = (T/m)·b₃ − g·e_z − (k_d/m)·|v|·v
q̇ = ½·q ⊗ (0, ω)
ω̇ = (ω_cmd − ω)/τ_att
```

Pilote automatique sur spline (Catmull-Rom paramétrée par longueur d'arc) :

```
v_d  = T_d · min(v_max, sqrt(a_lat_max/κ))
a_ff = v̇_d + v²·κ·n
b₃*  = normalize(a_ff + g·e_z)
ω_cmd = k_R·e_R + ω_ff       (asservissement d'attitude + anticipation)
```

Puis limitation de jerk et bruit de stick basse fréquence.

### 4.3 Paramètres exposés (sliders + valeurs)

| Paramètre | Plage | Défaut |
|---|---|---|
| Masse | 0,4–8 kg | 0,7 |
| Poussée max | 2–20 × m·g | 4 × m·g |
| Constante moteur | 0,02–0,10 s | 0,045 |
| Constante d'attitude | 0,02–0,05 s | 0,03 |
| Traînée | 0,002–0,02 | 0,006 |
| Inclinaison caméra (uptilt) | 20–45° | 30 |
| Rate centre / max | 80–250 / 500–900 °/s | 150 / 700 |
| Expo des rates | 0–0,7 | 0,35 |
| Jerk max | 200–800 °/s² | 400 |
| Accélération latérale max | 3–12 m/s² | 6 |
| Turbulence | 0,5–3 | 1,2 |
| Secousses | 0,02–0,4° RMS | 0,12 |
| Fréquence des secousses | 0,5–8 Hz | 3,5 |

### 4.4 Presets de vol

- **FPV cinelifter** : rates élevés, uptilt 30°, secousses modérées.
- **FPV freestyle** : rates maximaux, forte inertie, secousses marquées.
- **Hélicoptère** : constantes 0,15–0,4 s, dérapage en virage, flare à l'arrêt,
  vibrations basses fréquences.
- **Drone cinématique** : nacelle stabilisée (filtre 2ᵉ ordre), virages doux.
- **Racing** : trajectoire serrée, jerk élevé, pas de stabilisation.

### 4.5 Sorties

- Aperçu temps réel dans le viewport (basse résolution, ombres/AO mis en cache).
- Scrub sur la timeline, play/pause.
- Séquence PNG + MP4 (ffmpeg détecté ; sinon séquence PNG seule).
- Motion blur par **accumulation de 8–16 sous-images** dans l'intervalle
  d'obturation (180° par défaut) : c'est ce qui vend la vitesse.
- Export de la trajectoire en JSON/CSV (quaternions + tangentes) pour Blender.

### 4.6 Optimisation clé

Pour un survol, le soleil et le terrain sont fixes : **la shadow map et l'AO
sont indépendants de la caméra**. On les calcule une fois, on les réutilise pour
toutes les frames. Le coût par frame devient raster + shading seulement.

---

## 5. Proposition C — Interface en barres de fonctions

### 5.1 Principe

Pas de graphe. Une **barre horizontale de familles de fonctions** ; chaque
famille ouvre un **panneau latéral** (dock droit) composé de sections, chaque
réglage étant une ligne :

```
┌──────────────────────────────────────────────────────────────────┐
│ Terrain  Roche  Érosion  Eau  Végétation  Lumière  Caméra  Vol  Rendu │
├───────────────────────────────────────────┬──────────────────────┤
│                                           │ ROCHE — Grès          │
│                                           │ ─────────────────    │
│              Viewport                     │ Bancs                 │
│                                           │ Épaisseur  [ 26 ] m   │
│                                           │ ├────────●────────┤   │
│                                           │ Pendage    [  6 ] °   │
│                                           │ ├──●──────────────┤   │
│                                           │ Dureté     [0.80]     │
│                                           │ ├───────────●─────┤   │
│                                           │ ─────────────────    │
│                                           │ Fractures             │
│                                           │ Maille     [ 60 ] m   │
│                                           │ …                     │
├───────────────────────────────────────────┴──────────────────────┤
│ Pile : [Strates ✓] [Joints ✓] [Karst ✗] [Érosion ✓]   ⌄ détail   │
└──────────────────────────────────────────────────────────────────┘
```

Chaque ligne = libellé + slider + champ numérique éditable (cliquer pour taper,
molette pour ajuster, double-clic pour revenir au défaut). Unités affichées.

### 5.2 `ParamSpec` : une seule source de vérité

Chaque réglage est décrit une fois :

```rust
pub struct ParamSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub step: f64,
    pub tooltip: &'static str,
}
```

L'UI, le CLI (`--set roche.bancs.epaisseur=26`) et le MCP
(`geology.set`) génèrent leurs contrôles depuis cette table. Un seul endroit à
maintenir, et la documentation des paramètres peut être générée.

### 5.3 Pile de modificateurs (non destructrice)

Sous les panneaux, une pile ordonnée :

```
[Base: fBm alpin] → [Érosion hydraulique] → [Strates grès] → [Joints] → [Neige]
```

Chaque entrée : activer/désactiver, solo, dupliquer, supprimer, réordonner par
glisser, presets. Ce n'est **pas** un graphe : c'est une liste linéaire, comme
une pile d'effets photo. Recalcul uniquement des étages modifiés (cache par
étage), ce qui rend le réglage interactif.

### 5.4 Ce que ça change

- Ajouter un module = ajouter une famille + une `ParamSpec`, sans toucher au
  reste de l'UI.
- Les mêmes réglages sont scriptables (CLI/MCP) et documentés automatiquement.
- L'utilisateur voit toujours *où* il en est dans la chaîne de traitement.

---

## 6. Proposition D — Autres ajouts qui démarquent

Classés par rapport impact / effort.

| Ajout | Impact | Effort | Pourquoi |
|---|---|---|---|
| **Matériaux géologiques** (éboulis, neige, herbe, humidité) | très fort | moyen | c'est 70 % de l'écart avec la référence |
| **Détail de surface + normales triplanar** | très fort | faible | récupère le micro-relief sans géométrie |
| **Hydrologie** (rivières, lacs, bassins) | fort | moyen | l'eau plate actuelle trahit le rendu |
| **Export vidéo + motion blur** | fort | faible | rend le survol vendable |
| **Végétation instanciée** | fort | moyen | arbres/rochers, masques par pente/altitude |
| **Comparateur legacy** (DEM, palette 1994 vs moderne) | fort (niche) | faible | argument nostalgique unique |
| **Bake + hash + CLI batch** | moyen | faible | reproductibilité revendiquée |
| **Undo/redo** | moyen | moyen | confort, attendu par tout le monde |
| **Export glTF/Blender** | moyen | moyen | déjà au plan, à garder |
| **Mode 8 bits** | moyen (niche) | faible | fidélité au format legacy |

---

## 7. Écart avec l'image de référence : plan qualité

Objectif : s'approcher de la densité de détail et de la lecture géologique de
l'image fournie. Par ordre de gain :

1. **Résolution et détail** — passer les scènes de démonstration en 2050² (5 m
   par cellule), ajouter un détail fractal multi-échelle (0,5–8 m) et des
   normales de détail. C'est le gain le plus visible.
2. **Matériaux** — au lieu de 6 couleurs, un mélange pondéré :
   - roche (strates, joints, patine),
   - éboulis (pente + altitude + bruit, teinte claire),
   - neige (altitude + pente + azimut + bruit, plaques irrégulières),
   - herbe (replats, cuvettes, humidité),
   - humidité (bas de pente, proximité de l'eau).
3. **Lumière** — ombres plus nettes (résolution adaptative, PCF resserré),
   ambiante bleutée, occlusion cuite.
4. **Eau** — réflexion du ciel, écume, transparence par profondeur.
5. **Post** — micro-contraste, saturation sélective, netteté.

C'est ~10–15 jours de travail concentré, pas 30 semaines.

---

## 8. Feuille de route recalibrée

Hypothèse : itérations courtes, chacune livrable et testée.

| Étape | Contenu | Durée |
|---|---|---|
| **0. Socle** | `git init`, CI, gel du schéma v2, doc de paramètres | 0,5 j |
| **1. UI barres** | `ParamSpec`, barre de familles, dock droit, pile de modificateurs | 2 j |
| **2. Roche v1** | strates/joints/karst (fait), 7 presets, couleur, UI | 1,5 j |
| **3. Matériaux** | éboulis, neige, herbe, humidité, détail + normales | 3 j |
| **4. Vol v1** | spline, dynamique, rates, secousses, aperçu | 3 j |
| **5. Vidéo** | accumulation motion blur, séquence PNG, MP4 | 1,5 j |
| **6. Polish** | perf, tests, docs, démos | 2 j |

**Total : ~13,5 jours** pour une v0.2 nettement au-dessus de la v0.1, avec les
trois modules demandés.

---

## 9. Déjà implémenté (nuit du 13 novembre)

Les sections 3 à 5 sont désormais dans le code, voir `docs/ETAT_ET_REPRISE.md`
pour l'état exact et les commandes. Restent : qualité visuelle de proche
(triplanar, éboulis géométriques), waypoints de vol, cache shadow/AO, puis
végétation et export glTF.

Pour valider l'approche avant d'aller plus loin :

- crate `vf-geology` : strates pendées, joints orthogonaux/hexagonaux/radiaux,
  dissolution karstique, détail de surface, conscients de la résolution ;
- 7 presets de roche, validés et testés (déterminisme, non-régression) ;
- champ `geology` dans `Project` (schéma v2, migration des projets v1) ;
- `vistaforge terrain generate --rock <preset>` : érosion → géologie → passe de
  stabilisation ;
- coloration des bancs dans le renderer, cohérente avec la géométrie ;
- panneau « Rock / geology » dans l'UI avec sliders et valeurs numériques ;
- 23 nouveaux tests, `cargo test --workspace` vert.

Images de démonstration : `demo-out/geology/` (avant/après, gros plan, 2050²).

---

## 10. Décisions à prendre

1. **Ordre** : UI barres d'abord (tout s'y branche), ou matériaux d'abord
   (gain visuel immédiat) ? Recommandation : UI barres, puis matériaux.
2. **Résolution de travail** : 1026² pour l'interactif, 2050² pour le rendu
   final ? Recommandation : oui, avec un bouton « qualité finale ».
3. **Vidéo** : ffmpeg externe (simple, qualité) ou encodeur interne (zéro
   dépendance, plus de travail) ? Recommandation : ffmpeg détecté + séquence
   PNG de secours.
4. **Surplombs** : mesh local plus tard, ou jamais ? Recommandation : plus tard,
   après les matériaux.
5. **Git** : initialiser maintenant, avec un `.gitignore` propre (les DEM du
   corpus ne doivent pas être versionnés).

---

## Annexe — Références

- Recherche roches : [`docs/RECHERCHE_ROCHES_HEIGHTFIELD.md`](RECHERCHE_ROCHES_HEIGHTFIELD.md)
- Plan d'origine : non distribué.
- GPU Gems 3, ch. 1 — terrains procéduraux
- Musgrave, *Procedural Landscapes with Overhangs*
- Betaflight « Actual Rates » ; FAA *Helicopter Flying Handbook*, ch. 2
- RED, *Shutter Angle Tutorial*
