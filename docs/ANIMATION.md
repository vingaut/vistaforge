# Animation de caméra

> Un seul espace **Animation** remplace les anciens onglets **Vol** et
> **Animation**. Ce document décrit le parcours utilisateur, les interactions
> du viewport, les profils de vol et l'export.

![Tracé, cible et gizmo](screenshots/animation_path.png)

## Parcours en quatre étapes

L'espace Animation est découpé en quatre onglets numérotés. Le viewport et sa
barre de lecture restent visibles dans tous les onglets.

1. **Tracé** — dessiner et éditer le chemin de l'œil et la cible.
2. **Réglages** — durée, cadence, optique, contraintes.
3. **Profil** — choisir le corps et comparer les dynamiques.
4. **Lecture** — jouer l'animation, rendre une image ou exporter la séquence.

Le bouton de lecture n'est plus caché dans un panneau : une **barre de
transport** est ancrée en bas du viewport. `Lire`, `Pause`, `Début`, curseur de
temps, image précédente/suivante, boucle et bascule **Design / Caméra** y sont
toujours accessibles. `Espace` produit le même effet que `Lire`.

## 1. Tracé

Le tracé cyan est la trajectoire de l'œil. Les clés `E0`, `E1`… portent chacune
un temps et une position 3D. La cible orange matérialise le point regardé ; elle
n'est dessinée qu'en **vue design**, jamais dans le rendu.

### Dessiner

- Activez **Tracer** : chaque clic sur le terrain ajoute une clé à la fin du
  tracé et allonge la durée.
- L'altitude de la clé est réglée avant le clic (`+ 80 m` par défaut) ; un
  aperçu du point d'impact, de la verticale et de l'altitude suit la souris.
- **Ajouter Œil / Cible** choisit le chemin alimenté par le clic.
- **Échap** quitte le mode Tracer.

### Éditer

- Cliquez une clé dans le viewport ou dans la liste pour la sélectionner.
- Tirez la flèche **X** (rouge), **Y** (vert) ou **Z** (bleu) du gizmo. Le
  déplacement se fait dans le plan de l'écran le long de l'axe ; l'aimant
  arrondit la coordonnée en mètres (`0` = libre).
- **Insérer au temps courant** ajoute une clé sur la courbe sans déplacer les
  voisines ; **Suppr** ou **Retour arrière** supprime la clé sélectionnée.
- **Poignées automatiques** remet les poignées de la clé à zéro ; la courbe est
  alors lissée par ses voisines.
- **Cadrer le tracé** replace la caméra de conception sur le chemin ; le pivot d'orbite devient le centre du tracé, pas celui du terrain.
- Faire tourner la caméra de conception, zoomer ou recadrer n'altère jamais le
  tracé ; on revient à la vue caméra avec `Caméra`.

### Point de vue

La cible peut être :

| Mode | Comportement |
|---|---|
| **Fixe** | Un point immobile, utile pour un plan large ou une orbite. |
| **Tracé animé** | Une seconde courbe avec ses clés `T0`, `T1`… |
| **Avance** | La caméra regarde devant elle ; `lead` et lissage réglables. |
| **Manuel** | La caméra suit son axe, sans cible matérialisée. |

Le viewport de conception dessine le **viseur** du point de vue (tronc de
frustum depuis l'œil vers la cible) et la ligne œil → cible. Le rendu utilise
exactement la caméra échantillonnée à `t`, sans ces overlays.

## 2. Réglages

- **Durée** (0,1–600 s), **images/s** (1–120), **nombre d'images**.
- **Optique** : FOV, profondeur de champ, ouverture, mise au point, roulis,
  liaison caméra rigide ou ressort.
- **Contraintes** : hauteur au sol, sol de référence, distance aux parois,
  évitement, douceur, application à l'œil / la caméra / la cible.

Un encadré de calage compare **vitesse demandée** et **vitesse de croisière**
du profil (75 % de sa vitesse maximale). Si la durée est trop courte, le corps
sature et prend du retard ; le bouton **Ajuster la durée** applique la durée
conseillée calculée depuis la longueur du tracé. Une durée plus longue ne
déplace aucune clé : elle change seulement la vitesse de parcours.

## 3. Profil de vol

Le profil ne change pas le tracé ; il change la façon dont le corps le suit.

| Preset | Caractère |
|---|---|
| **Drone FPV** | Nerveux, accélérations franches, virages serrés, forte inclinaison. |
| **Oiseau** | Souple et flottant, réactions moyennes, turbulences. |
| **Hélicoptère** | Posé et lourd, accélérations douces, faible roulis, nacelle stable. |
| **Ballon dirigeable** | Très inertiel, grandes courbes, roulis presque nul. |
| **Avion de chasse** | Ultra rapide, réponses immédiates, virages violents. |
| **Manuel** | Suit exactement le tracé, sans inertie ; exclu des comparaisons. |

Chaque preset ajuste aussi l'optique et la garde au sol (FOV, ouverture,
clearance), ce qui aide à ressentir le véhicule.

- Les **courbes** comparent vitesse et roulis des six corps sur le tracé
  courant. Manuel est exclu de l'échelle.
- **Comparer les trajectoires** superpose les cinq corps dans le viewport ; le
  corps actif est en jaune, les autres gardent leur couleur.
- **Tracé de démonstration (S)** crée un S dont la taille et la durée sont
  adaptées au profil (90 % de sa vitesse max) et cadre la caméra dessus. C'est
  le moyen le plus rapide de voir la différence entre un drone et un
  hélicoptère en quelques secondes.
- **Tester ce profil** passe en vue caméra et lance la lecture depuis le début.

Le rig utilise un ressort critique avec **avance de vitesse** : l'accélération
demandée est bornée par axe, puis le jerk est limité. Sans cette avance, le
ressort se battait contre la vitesse du tracé et, une fois la borne
d'accélération atteinte, les corps rapides oscillaient autour de la courbe au
lieu de la suivre.

## 4. Lecture et export

- Curseur de temps : place la caméra à l'instant choisi.
- `Lire` / `Pause`, boucle, vitesse de lecture (0,05–4×).
- **Vue design** : tracé, cible, viseur et gizmo.
- **Vue caméra** : le cadre réel ; orbiter ou zoomer depuis cette vue repasse
  automatiquement en design.
- **Image à ce temps (CPU)** : rendu final d'une frame.
- **Séquence PNG** : `frame_00000.png`, `frame_00001.png`… dans le dossier
  choisi, avec progression et annulation. La résolution et la qualité viennent
  du panneau **Rendu**. Un export de 1200 images en pleine résolution peut être
  long ; commencez par 320×180 en qualité Low.

L'encodeur MP4 reste disponible en CLI :

```bash
vistaforge anim render scene.vfproj frames/ --video clip.mp4
```

## CLI

```bash
# État du rig
vistaforge rig info scene.vfproj

# Preset de profil (sans toucher au tracé)
vistaforge rig preset scene.vfproj helicopter

# Clé de tracé
vistaforge rig key scene.vfproj --time 4 --x 3200 --y 5100 --z 900

# Une image et une séquence avec le rig du projet
vistaforge anim still scene.vfproj frame.png --time 2
vistaforge anim render scene.vfproj frames/ --fps 24
```

## Limites connues

- Les poignées Bézier manuelles ne sont pas encore éditables dans le viewport ;
  le bouton **Poignées automatiques** remet la clé sélectionnée à zéro et laisse
  la courbe lissée par ses voisines.
- L'évitement de parois est un champ de distance 2D : il ignore l'altitude. Sur
  un tracé qui survole une falaise, il peut pousser latéralement alors qu'il n'y
  a pas de collision. Les comparaisons de profils désactivent cet évitement pour
  isoler la dynamique.
- Le DOF est calculé par le rig et appliqué par Blender/Cycles ; le rasteriseur
  CPU ne le rasterise pas.

## Captures

Les captures de cette page proviennent du mode capture :

```bash
VISTAFORGE_CAPTURE=/tmp/vf-captures \
  cargo run -p vf-app -- demo-out/geology/alpine.vfproj

VISTAFORGE_CAPTURES=/tmp/vf-captures \
VISTAFORGE_VIEWPORT=360,100,1270,810 \
  python3 scripts/make_help_screenshots.py
```

Voir [`AIDE.md`](AIDE.md) pour la fabrication des fiches.
