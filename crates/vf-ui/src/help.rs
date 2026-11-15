//! Contextual help overlay.
//!
//! Controls register their rectangle and a [`HelpTopic`] during the frame.
//! After the panels are built, the topmost hovered topic is resolved and shown
//! as an annotated card inside the viewport. The card is a small tutorial, not
//! a tooltip: a summary, the few parameters that really change the result, an
//! annotated screenshot of the expected effect and live values for the current
//! scene. The card can be pinned and browsed like a table of contents.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use vf_core::{CameraRig, TargetMode};

use crate::animation_ui::TimingFit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpTopic {
    Workspace,
    Navigation,
    Transport,
    Path,
    Target,
    Timing,
    Optics,
    Constraints,
    Profile,
    Preview,
    Export,
    Terrain,
    World,
    Modifiers,
    Rock,
    Detail,
    Sculpt,
    Erosion,
    Water,
    Materials,
    Light,
    Camera,
    RenderFinal,
    Adaptive,
    Gpu,
}

impl HelpTopic {
    pub const ALL: [Self; 25] = [
        Self::Workspace,
        Self::Navigation,
        Self::Transport,
        Self::Path,
        Self::Target,
        Self::Timing,
        Self::Optics,
        Self::Constraints,
        Self::Profile,
        Self::Preview,
        Self::Export,
        Self::Terrain,
        Self::World,
        Self::Modifiers,
        Self::Rock,
        Self::Detail,
        Self::Sculpt,
        Self::Erosion,
        Self::Water,
        Self::Materials,
        Self::Light,
        Self::Camera,
        Self::RenderFinal,
        Self::Adaptive,
        Self::Gpu,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Les espaces de travail",
            Self::Navigation => "Naviguer dans la scène",
            Self::Transport => "Lire et régler le temps",
            Self::Path => "Tracé de la caméra",
            Self::Target => "Point de vue (cible)",
            Self::Timing => "Durée, cadence et nombre d'images",
            Self::Optics => "Optique de la caméra",
            Self::Constraints => "Contraintes de vol",
            Self::Profile => "Profil de vol",
            Self::Preview => "Lecture de l'animation",
            Self::Export => "Fichiers, export et Blender",
            Self::Terrain => "Génération du terrain",
            Self::World => "Monde et tuilage",
            Self::Modifiers => "Modificateurs",
            Self::Rock => "Géologie et roche",
            Self::Detail => "Détail de surface",
            Self::Sculpt => "Sculpter le relief",
            Self::Erosion => "Érosion",
            Self::Water => "Mer et eau",
            Self::Materials => "Matières et étagement",
            Self::Light => "Lumière et ciel",
            Self::Camera => "Caméra de conception",
            Self::RenderFinal => "Rendu final CPU",
            Self::Adaptive => "Aperçu adaptatif",
            Self::Gpu => "Viewport GPU",
        }
    }

    pub fn section(self) -> &'static str {
        match self {
            Self::Workspace | Self::Navigation | Self::Transport => "Prise en main",
            Self::Path
            | Self::Target
            | Self::Timing
            | Self::Optics
            | Self::Constraints
            | Self::Profile
            | Self::Preview
            | Self::Export => "Animation",
            Self::Terrain
            | Self::World
            | Self::Modifiers
            | Self::Rock
            | Self::Detail
            | Self::Sculpt
            | Self::Erosion
            | Self::Water
            | Self::Materials
            | Self::Light
            | Self::Camera => "Terrain et scène",
            Self::RenderFinal | Self::Adaptive | Self::Gpu => "Rendu",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Workspace => {
                "Chaque espace de la barre du haut regroupe un métier. Le panneau de gauche ne montre que les réglages utiles à cet espace ; le viewport et le projet restent les mêmes."
            }
            Self::Navigation => {
                "Le cube donne l'orientation de la scène. Cliquez une face ou un axe pour cadrer, la maison pour revenir au cadrage par défaut."
            }
            Self::Transport => {
                "La barre de lecture reste ancrée en bas du viewport : elle permet de lancer l'animation depuis n'importe quel onglet, sans chercher un bouton dans le panneau."
            }
            Self::Path => {
                "Le tracé cyan est la trajectoire de l'œil. Les clés E0, E1… portent chacune un temps et une position 3D ; la courbe passe par toutes."
            }
            Self::Target => {
                "La cible orange matérialise ce que la caméra regarde. Elle n'existe que dans la vue design : fixe, en avance, en décalage rigide, en retard ou sur son propre tracé. Ces modes changent le lien caméra/cible, pas la trajectoire de l'œil."
            }
            Self::Timing => {
                "La durée, les images/s et le nombre d'images déterminent la longueur du clip. La durée ne dilate pas le tracé : elle change la vitesse à laquelle on le parcourt."
            }
            Self::Optics => {
                "Le FOV, la profondeur de champ, le roulis et la liaison ressort changent ce que la caméra voit, sans modifier le tracé."
            }
            Self::Constraints => {
                "Les contraintes corrigent automatiquement la trajectoire simulée : hauteur au-dessus du sol, plan de référence fixe et évitement des parois raides. Elles s'appliquent à l'œil, à la caméra ou à la cible."
            }
            Self::Profile => {
                "Le profil ne change pas le tracé : il change la façon dont le corps suit ce tracé (inertie, accélérations, roulis, secousses). C'est là que se joue la différence drone / hélicoptère / dirigeable."
            }
            Self::Preview => {
                "La lecture anime la caméra dans le viewport. On peut régler le temps à la main, changer la vitesse et basculer entre vue design et vue caméra."
            }
            Self::Export => {
                "L'espace Fichiers échange le terrain (GeoTIFF, ASCII, DEM, PNG/JPEG), exporte la scène pour Blender (GLB + textures) et les trajectoires seules (.vfcam, GLB, CSV). La séquence PNG reste dans l'onglet Lecture."
            }
            Self::Terrain => {
                "Génère un nouveau relief à partir d'un mode, d'une taille, d'une graine et d'un preset. La génération remplace le heightfield courant."
            }
            Self::World => {
                "Compose le terrain en monde unique, tuilé ou sans couture, et applique une variation macro pour éviter la répétition."
            }
            Self::Modifiers => {
                "Les modificateurs forment une pile non destructive appliquée au rendu. L'ordre compte : un modificateur peut annuler ou révéler l'effet du précédent."
            }
            Self::Rock => {
                "La géologie façonne des bancs, des fractures et des dissolutions. Elle est appliquée après l'érosion, puis stabilisée, pour que les corniches ne soient pas arrondies."
            }
            Self::Detail => {
                "Le détail ajoute un relief fractal évalué au-dessus du heightfield, avec un maillage plus fin et un patch de gros plan. C'est ce qui donne du grain à quelques mètres."
            }
            Self::Sculpt => {
                "Le pinceau sculpte directement le heightfield. Chaque trait est annulable et ne touche que la région concernée ; la densification et l'aperçu supersamplé révèlent les fins de banc."
            }
            Self::Erosion => {
                "L'érosion hydraulique creuse des ravines et dépose des sédiments ; l'érosion thermique adoucit les pentes. Elle précède la géologie."
            }
            Self::Water => {
                "L'eau est un plan horizontal au niveau de mer choisi. Vagues, opacité et écume se voient surtout près des côtes."
            }
            Self::Materials => {
                "Les matières sont des masques procéduraux : roche, éboulis, neige, végétation et humidité. L'étagement règle les altitudes de la neige, des arbres et de la plage."
            }
            Self::Light => {
                "Le soleil, la palette, l'atmosphère et l'exposition décident de l'heure et de l'ambiance. Une palette ne change ni la géométrie ni les matières."
            }
            Self::Camera => {
                "La caméra de conception sert à tourner autour de la scène. Elle est indépendante de la caméra animée, qui est échantillonnée depuis le tracé."
            }
            Self::RenderFinal => {
                "Le bouton Render produit l'image finale CPU, déterministe, indépendante du viewport GPU. C'est aussi ce rendu qui sert à l'export de séquence."
            }
            Self::Adaptive => {
                "Pendant une interaction, l'aperçu CPU baisse sa résolution puis se raffine dès que la main s'arrête, pour rester fluide sans sacrifier l'image finale."
            }
            Self::Gpu => {
                "Le viewport GPU dessine le relief en temps réel via wgpu (Vulkan ou Metal). C'est un aperçu de navigation : il ne modifie jamais le rendu final."
            }
        }
    }

    pub fn bullets(self) -> &'static [&'static str] {
        match self {
            Self::Workspace => &[
                "Animation réunit désormais tracé, réglages, profil de vol et lecture : l'ancien onglet Vol a disparu.",
                "Changer d'espace ne change pas le projet, seulement les outils affichés.",
                "Survolez les titres de section : chaque fiche explique les paramètres qui comptent vraiment.",
            ],
            Self::Navigation => &[
                "1 = face, 2 = dessus, 3 = droite, 0 ou Home = cadrage par défaut.",
                "Glisser dans le viewport orbite ; la molette zoome ; le double-clic recadre.",
                "Les faces du cube sont cliquables, y compris les axes négatifs.",
            ],
            Self::Transport => &[
                "▶ Lire lit dans la vue courante : design (frustum et marqueurs animés) ou caméra (cadre rendu). Espace fait la même chose.",
                "Le curseur de temps place l'animation à l'instant choisi ; − et + avancent d'une image.",
                "Design montre le tracé et le gizmo ; Caméra montre le cadre réel.",
                "Timeline ouvre le dock de pistes et le graph editor ; ⟲ active la boucle.",
                "Un bandeau signale quand le profil est loin derrière le tracé : allongez la durée, mettez l'ease à 0 ou passez en Manuel.",
            ],
            Self::Path => &[
                "Tracer : cliquez sur le terrain ; chaque clic ajoute une clé et allonge la durée. L'altitude est réglable avant le clic.",
                "Trois méthodes équivalentes : gizmo X/Y/Z, glisser libre de la clé dans le viewport, ou valeurs numériques.",
                "Glisser libre : plan de l'écran ; Maj = vertical seul ; Alt = poser la clé sur le terrain.",
                "Timeline : glisser un losange change son temps ; double-clic insère une clé ; clic droit supprime.",
            ],
            Self::Target => &[
                "Fixe : un point immobile, utile pour un plan large ou une orbite.",
                "Avance : la caméra regarde devant elle (lead et lissage réglables).",
                "Décalage : la cible reste à un offset monde constant de l'œil (liaison rigide, sans retard).",
                "Retard : la cible vise l'œil tel qu'il était il y a N secondes (regard arrière).",
                "Tracé animé : une seconde courbe, avec ses propres clés T0, T1… Copier l'œil ou synchroniser les temps crée un lien éditable.",
                "Manuel : la caméra suit son axe, sans cible matérialisée.",
            ],
            Self::Timing => &[
                "Images = durée × images/s. À 24 fps, 8 s font 192 images.",
                "Une durée trop courte demande une vitesse que le corps ne peut pas atteindre : il sature et rate les virages.",
                "Le bouton « Durée conseillée » calcule une durée à partir de la longueur du tracé et de la vitesse de croisière du profil.",
                "Par défaut, la durée étire les temps des clés (positions 3D inchangées) : le tracé occupe tout le clip. Décochez « Étirer les clés » pour garder des temps absolus.",
                "« Suivre exactement le tracé » (coché) garantit vitesse = distance / temps et clé atteinte à son temps. Décochez-le pour l'inertie du profil.",
                "La durée conseillée inclut 15 % de marge pour l'accélération et la stabilisation finale.",
            ],
            Self::Optics => &[
                "Un FOV large (70–90°) accélère la sensation de vitesse ; un FOV serré (25–40°) comprime la scène.",
                "La profondeur de champ est calculée par le rig, mais rasterisée seulement par Blender/Cycles.",
                "La liaison Ressort fait traîner la caméra derrière l'œil : utile pour un plan embarqué souple.",
                "Le roulis explicite s'ajoute au roulis automatique calculé depuis l'accélération latérale.",
            ],
            Self::Constraints => &[
                "Hauteur sol : distance minimale au-dessus du terrain ; la correction remonte l'œil sans le traverser.",
                "Distance parois : le rig échantillonne un champ de distance aux pentes > 45° et pousse latéralement.",
                "Sol de référence fixe : utile sans heightfield ou pour éviter une mer de nuages.",
                "Douceur : 0 = correction dure, 1 = correction très molle. Appliquer à la cible évite de traverser un mur en visée.",
            ],
            Self::Profile => &[
                "Drone FPV : nerveux, accélérations franches, virages serrés, forte inclinaison.",
                "Hélicoptère : posé et lourd, accélérations douces, faible roulis, nacelle stable.",
                "Dirigeable : très inertiel, grandes courbes, roulis presque nul.",
                "« Comparer les trajectoires » superpose les cinq corps sur le même tracé.",
                "« Tracé de démonstration » crée un S adapté au profil, à 90 % de sa vitesse max, pour voir la différence en quelques secondes.",
            ],
            Self::Preview => &[
                "Espace = lecture / pause. En vue design, l'animation se lit sans quitter l'orbite.",
                "Vitesse ralentit ou accélère la lecture ; elle ne change pas la physique du clip.",
                "En vue caméra, orbiter ou zoomer repasse automatiquement en vue design.",
                "Le graph editor trace vitesse et roulis du profil sélectionné sur la même timeline.",
            ],
            Self::Export => &[
                "Terrain : GeoTIFF 32 bits (DEM géoréférencé), ESRI ASCII Grid, DEM v1, PNG 16 bits + sidecar JSON, JPEG de contrôle.",
                "Heightmap : PNG, TIFF, JPEG ; la plage min/max ou le sidecar fixe l'altitude, 0/0 = automatique.",
                "Blender : dossier scene.glb + textures + scene.json, ou addon bridge/blender/vistaforge_io.py pour les trajectoires.",
                "Animation seule : .vfcam (rig complet), .glb (caméra + cible, réimportable dans Blender) ou .csv.",
            ],
            Self::Terrain => &[
                "Classic est un relief fractal direct ; Modern combine des familles de formes.",
                "La taille augmente le coût quadratiquement : 514² pour travailler, 1026² ou plus pour un rendu final.",
                "La graine rend la génération reproductible ; Random en choisit une nouvelle.",
                "Générer remplace le heightfield courant : sauvegardez ou dupliquez avant.",
            ],
            Self::World => &[
                "Single : une seule tuile. Tiled : répétition avec rotations et miroirs.",
                "Seamless corrige les bords de la tuile par une rampe localisée (continuité C0).",
                "La variation macro casse la répétition à grande échelle.",
                "Recomposer un projet déjà composé tuile à nouveau le monde : repartez d'une tuile.",
            ],
            Self::Modifiers => &[
                "Ajoutez un opérateur, réglez ses paramètres, réordonnez-le ou désactivez-le sans le supprimer.",
                "L'ordre est essentiel : un lissage après une crête efface la crête.",
                "« Cuire » applique la pile au heightfield et vide la pile : irréversible sans sauvegarde.",
                "Exemples : Terrasses pour des bancs, Érosion pour des ravines, Snow pour des congères.",
            ],
            Self::Rock => &[
                "Le preset choisit une famille (granite, calcaire, basalte…), puis les bancs règlent l'épaisseur, le pendage et l'azimut.",
                "Les fractures dessinent des réseaux orthogonaux, hexagonaux ou radiaux.",
                "La dissolution creuse des dolines et des cannelures verticales.",
                "L'opérateur Canyon ajoute un méandre à fond plat ; la palette assortie est désactivée par défaut.",
            ],
            Self::Detail => &[
                "Le profil de roche choisit les couches de détail (sillons, arêtes, cratères, exfoliation).",
                "La densité du maillage subdivise la grille de rendu (1× à 8×) sans changer le heightfield.",
                "Le patch de gros plan ajoute un maillage fin à 2–50 cm de pas sous la caméra.",
                "Trop de détail à distance produit du bruit : la montée en fréquence est limitée par l'empreinte pixel.",
            ],
            Self::Sculpt => &[
                "Le module travaille à la résolution du heightfield : à 25 m/cellule, un banc de 8 m ne peut pas être sculpté. Densifiez ×2 avant les terrasses fines.",
                "Terrasse : les bancs suivent un pendage, avec une part de banc plat et un riser raide. Les orgues verticales creusent les risers.",
                "Orgues de pente : sillons alignés sur la descente, pour une paroi basaltique. Intensité = profondeur.",
                "L'aperçu 2×–3× lisse les facettes sans changer le terrain ; l'exagération règle le relief ombré.",
                "Chaque trait garde une copie de la région avant/après : Ctrl+Z / Ctrl+Y.",
            ],
            Self::Erosion => &[
                "Les gouttes creusent des ravines ; le dépôt et l'inertie règlent les dépôts en pied de pente.",
                "L'érosion thermique limite les pentes au talus : augmentez les itérations pour adoucir.",
                "Érodée à l'excès, la scène perd ses corniches : la géologie est appliquée après.",
                "La génération avec érosion est plus lente qu'un simple aperçu.",
            ],
            Self::Water => &[
                "Le niveau de mer est une altitude absolue : comparez-le au relief affiché en bas du panneau.",
                "L'opacité et la couleur de l'eau se lisent surtout sur les hauts-fonds.",
                "Écume et vagues animent la surface dans le rendu final et le viewport GPU.",
                "Désactiver l'eau est le moyen le plus rapide de vérifier une vallée.",
            ],
            Self::Materials => &[
                "Neige et végétation sont des masques : couverture, pente et altitude se combinent.",
                "L'éboulis apparaît sur les pentes intermédiaires, pas sur les parois ni les replats.",
                "Le détail de normale ajoute du grain à la roche ; l'échelle est en mètres.",
                "L'étagement règle la ligne de neige, la limite des arbres et la plage.",
            ],
            Self::Light => &[
                "La palette donne l'heure et l'ambiance ; elle ne change pas la géométrie.",
                "L'azimut et la déclinaison du soleil déplacent les ombres et révèlent les faces.",
                "L'exposition est un multiplicateur global ; l'ambiante éclaire les ombres.",
                "Le preset dusk ajoute un soleil rasant et des étoiles.",
            ],
            Self::Camera => &[
                "Le gizmo d'orientation et les vues CAO cadrent la scène sans toucher au projet.",
                "La molette zoome, le glisser orbite, le double-clic recadre.",
                "La caméra de conception est mémorisée quand vous passez en vue caméra animée.",
                "Les raccourcis 1/2/3/0 fonctionnent même quand le viewport n'a pas le focus.",
            ],
            Self::RenderFinal => &[
                "Fast, Refined et Photoreal règlent SSAA, ombres, AO et subdivision.",
                "Le rendu CPU est multi-thread et déterministe : deux rendus identiques donnent le même PNG.",
                "Le viewport GPU ne modifie jamais l'image exportée.",
                "Une séquence d'animation utilise exactement ce rendu, image par image.",
            ],
            Self::Adaptive => &[
                "Pendant un glisser, l'aperçu descend à 25–50 % puis remonte à 100 % après ~200 ms.",
                "L'échelle interactive est apprise du temps mesuré pour viser ~45 ms par passe.",
                "Décochez Aperçu adaptatif pour forcer la pleine résolution (plus lent).",
                "Le HUD affiche la résolution et le temps de la dernière passe.",
            ],
            Self::Gpu => &[
                "Basculez dans Rendu → Vue 3D GPU ; le tracé, la cible et le gizmo restent des overlays egui.",
                "Pas encore d'ombres, d'AO ni de nuages volumétriques dans cet aperçu.",
                "En cas d'échec Vulkan/Metal, lancez avec VISTAFORGE_RENDERER=glow.",
                "Le GPU sert à naviguer ; le bouton Render reste le rendu de référence.",
            ],
        }
    }

    pub fn image(self) -> Option<HelpImage> {
        Some(match self {
            Self::Workspace => HelpImage::Workspace,
            Self::Navigation => HelpImage::Gizmo,
            Self::Transport | Self::Preview => HelpImage::Transport,
            Self::Path => HelpImage::Path,
            Self::Target => HelpImage::Target,
            Self::Timing => HelpImage::Timing,
            Self::Optics => HelpImage::Optics,
            Self::Constraints => HelpImage::Constraints,
            Self::Profile => HelpImage::Profile,
            Self::Export | Self::RenderFinal | Self::Adaptive | Self::Gpu => HelpImage::Render,
            Self::Terrain => HelpImage::Terrain,
            Self::World => HelpImage::World,
            Self::Modifiers => HelpImage::Modifiers,
            Self::Rock => HelpImage::Rock,
            Self::Detail => HelpImage::Detail,
            Self::Sculpt => HelpImage::Sculpt,
            Self::Erosion => HelpImage::Erosion,
            Self::Water => HelpImage::Water,
            Self::Materials => HelpImage::Materials,
            Self::Light => HelpImage::Light,
            Self::Camera => HelpImage::Camera,
        })
    }

    pub fn caption(self) -> &'static str {
        match self {
            Self::Workspace => {
                "Effet attendu : le même terrain, mais un panneau et des outils adaptés à la tâche."
            }
            Self::Navigation => {
                "Effet attendu : la scène est cadrée sur la face choisie, sans perdre la caméra animée."
            }
            Self::Transport => {
                "Effet attendu : la lecture démarre immédiatement, depuis n'importe quel onglet."
            }
            Self::Path => {
                "Effet attendu : la courbe se déforme sur l'axe tiré ; le viseur et la cible suivent la clé."
            }
            Self::Target => {
                "Effet attendu : la ligne œil → cible et le cadre du viseur montrent exactement le plan filmé."
            }
            Self::Timing => {
                "Effet attendu : à durée courte le corps sature et coupe les virages ; à durée longue le vol est fluide."
            }
            Self::Optics => {
                "Effet attendu : le FOV change la sensation de vitesse, le ressort ajoute du retard, le roulis incline l'horizon."
            }
            Self::Constraints => {
                "Effet attendu : la trajectoire simulée se décale pour garder la distance au sol et aux parois, sans modifier les clés."
            }
            Self::Profile => {
                "Effet attendu : à tracé égal, le drone coupe et penche, l'hélicoptère prend large et reste à plat, le dirigeable dérive."
            }
            Self::Preview => {
                "Effet attendu : la caméra avance dans le viewport et le temps se met à jour dans la barre."
            }
            Self::Export => {
                "Effet attendu : un dossier rempli de frames PNG numérotées, prêtes à encoder."
            }
            Self::Terrain => {
                "Effet attendu : un nouveau relief, avec la même caméra et les mêmes réglages de scène."
            }
            Self::World => {
                "Effet attendu : la tuile se répète sans bord visible, avec une variation macro."
            }
            Self::Modifiers => {
                "Effet attendu : chaque opérateur laisse une signature visible (marches, ravines, congères…)."
            }
            Self::Rock => {
                "Effet attendu : des bancs inclinés, des corniches et des cannelures verticales."
            }
            Self::Detail => {
                "Effet attendu : la roche gagne du grain et des aspérités à l'échelle métrique."
            }
            Self::Sculpt => {
                "Effet attendu : des bancs stratifiés à risers raides, des orgues verticales, et un aperçu assez net pour les distinguer."
            }
            Self::Erosion => {
                "Effet attendu : des ravines ramifiées et des dépôts en pied de pente."
            }
            Self::Water => {
                "Effet attendu : un plan d'eau horizontal qui noie les points bas et révèle les hauts-fonds."
            }
            Self::Materials => {
                "Effet attendu : neige, éboulis et végétation suivent l'altitude et la pente."
            }
            Self::Light => {
                "Effet attendu : une heure et une ambiance différentes, avec des ombres et une palette cohérentes."
            }
            Self::Camera => {
                "Effet attendu : un cadrage CAO net, réversible, sans modifier le tracé animé."
            }
            Self::RenderFinal => {
                "Effet attendu : une image CPU déterministe, plus détaillée que l'aperçu GPU."
            }
            Self::Adaptive => {
                "Effet attendu : l'aperçu reste réactif pendant le glisser, puis se raffine tout seul."
            }
            Self::Gpu => {
                "Effet attendu : un viewport fluide pour tourner autour du relief, sans attendre le CPU."
            }
        }
    }

    pub fn try_it(self) -> &'static str {
        match self {
            Self::Path => {
                "À essayer : Tracer, trois clics sur la crête, puis tirez la flèche Z de la dernière clé."
            }
            Self::Target => {
                "À essayer : passez en cible Tracé animé, puis déplacez T1 à l'opposé de E1."
            }
            Self::Timing => {
                "À essayer : réglez 4 s, puis cliquez Durée conseillée et comparez la lecture."
            }
            Self::Constraints => {
                "À essayer : montez la hauteur sol à 120 m et observez la trajectoire jaune se décaler."
            }
            Self::Profile => {
                "À essayer : Tracé de démonstration, puis Comparez drone et hélicoptère en vue design."
            }
            Self::Transport => "À essayer : Espace, puis − / + pour vérifier une frame précise.",
            Self::Export => {
                "À essayer : 320×180 en qualité Low dans un dossier temporaire avant un export final."
            }
            Self::Sculpt => {
                "À essayer : densifiez, preset Terrasses basaltiques, trait horizontal, puis Orgues de pente sur le riser."
            }
            Self::Detail => "À essayer : activez le patch de gros plan et zoomez sur une paroi.",
            Self::Gpu => {
                "À essayer : orbitez en vue GPU, puis cliquez Render : l'image CPU reste la référence."
            }
            _ => "",
        }
    }

    fn live_line(self, context: &HelpContext<'_>) -> Option<String> {
        match self {
            Self::Path => Some(format!(
                "{} clés · {:.0} m de tracé · {:.2} s · cible {}",
                context.rig.eye.keys.len(),
                vf_flight::rig::path_length(&context.rig.eye),
                context.rig.duration_s,
                target_mode_label(context.rig.target.mode)
            )),
            Self::Target => Some(format!(
                "Mode {} · {} clés de cible · avance {:.2} s",
                target_mode_label(context.rig.target.mode),
                context.rig.target.path.keys.len(),
                context.rig.target.lead_s
            )),
            Self::Timing => {
                let fit = TimingFit::measure(context.rig);
                Some(format!(
                    "{} images · {:.0} m/s demandés pour {:.0} m/s de croisière",
                    context.rig.frame_count(),
                    fit.required_speed_mps,
                    fit.cruise_speed_mps
                ))
            }
            Self::Profile => {
                let fit = TimingFit::measure(context.rig);
                Some(format!(
                    "{} · {} clés · {} images · {}",
                    context.rig.body.preset.label(),
                    context.rig.eye.keys.len(),
                    context.rig.frame_count(),
                    if fit.is_strained() {
                        "durée tendue"
                    } else {
                        "durée confortable"
                    }
                ))
            }
            Self::Preview | Self::Transport => Some(format!(
                "{} · t={:.2}/{:.2} s · {} fps · vue {}",
                if context.playing { "lecture" } else { "pause" },
                context.time_s,
                context.rig.duration_s,
                context.rig.fps,
                context.view_mode
            )),
            Self::Constraints => Some(format!(
                "sol {:.0} m · parois {:.0} m · {} · {}",
                context.rig.constraints.ground_clearance_m,
                context.rig.constraints.wall_distance_m,
                if context.rig.constraints.enabled {
                    "actives"
                } else {
                    "inactives"
                },
                if context.rig.constraints.wall_avoidance {
                    "évitement on"
                } else {
                    "évitement off"
                }
            )),
            Self::Optics => Some(format!(
                "FOV {:.0}° · f/{:.1} · DOF {} · liaison {}",
                context.rig.optics.fov_y_deg,
                context.rig.optics.fstop,
                if context.rig.optics.dof_enabled {
                    "on"
                } else {
                    "off"
                },
                match context.rig.optics.link {
                    vf_core::CameraLink::Eye => "rigide",
                    vf_core::CameraLink::Spring => "ressort",
                }
            )),
            _ => None,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|topic| {
            topic.title().to_ascii_lowercase() == normalized
                || format!("{topic:?}").to_ascii_lowercase() == normalized
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpImage {
    Workspace,
    Gizmo,
    Path,
    Target,
    Timing,
    Optics,
    Constraints,
    Profile,
    Transport,
    Terrain,
    World,
    Modifiers,
    Rock,
    Detail,
    Sculpt,
    Erosion,
    Water,
    Materials,
    Light,
    Camera,
    Render,
}

impl HelpImage {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Workspace => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/workspace.png"
            )),
            Self::Gizmo => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/gizmo.png"
            )),
            Self::Path => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/path.png"
            )),
            Self::Target => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/target.png"
            )),
            Self::Timing => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/timing.png"
            )),
            Self::Optics => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/optics.png"
            )),
            Self::Constraints => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/constraints.png"
            )),
            Self::Profile => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/profile.png"
            )),
            Self::Transport => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/transport.png"
            )),
            Self::Terrain => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/terrain.png"
            )),
            Self::World => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/world.png"
            )),
            Self::Modifiers => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/modifiers.png"
            )),
            Self::Rock => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/rock.png"
            )),
            Self::Detail => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/detail.png"
            )),
            Self::Sculpt => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/sculpt.png"
            )),
            Self::Erosion => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/erosion.png"
            )),
            Self::Water => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/water.png"
            )),
            Self::Materials => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/materials.png"
            )),
            Self::Light => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/light.png"
            )),
            Self::Camera => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/camera.png"
            )),
            Self::Render => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/help/render.png"
            )),
        }
    }
}

/// Live values shown inside the help card.
pub struct HelpContext<'a> {
    pub rig: &'a CameraRig,
    pub playing: bool,
    pub time_s: f32,
    pub view_mode: &'a str,
}

pub struct HelpState {
    pub enabled: bool,
    pub pinned: bool,
    shown: Option<HelpTopic>,
    hovered: Option<HelpTopic>,
    registered: Vec<(Rect, HelpTopic)>,
    last_rect: Rect,
    textures: std::collections::HashMap<HelpImage, egui::TextureHandle>,
}

impl Default for HelpState {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpState {
    pub fn new() -> Self {
        Self {
            enabled: true,
            pinned: false,
            shown: None,
            hovered: None,
            registered: Vec::new(),
            last_rect: Rect::NOTHING,
            textures: std::collections::HashMap::new(),
        }
    }

    /// Open a topic and keep it visible (used by F1 and by the screenshot
    /// capture mode).
    pub fn reveal(&mut self, topic: HelpTopic) {
        self.enabled = true;
        self.pinned = true;
        self.shown = Some(topic);
        self.hovered = Some(topic);
    }

    pub fn close(&mut self) {
        self.enabled = false;
        self.pinned = false;
        self.shown = None;
        self.hovered = None;
        self.last_rect = Rect::NOTHING;
    }

    pub fn toggle(&mut self, fallback: HelpTopic) {
        if self.enabled {
            self.close();
        } else {
            self.enabled = true;
            self.reveal(fallback);
        }
    }

    pub fn is_open(&self) -> bool {
        self.enabled && self.shown.is_some()
    }

    pub fn begin_frame(&mut self) {
        self.registered.clear();
    }

    pub fn register(&mut self, rect: Rect, topic: HelpTopic) {
        if rect.is_positive() {
            self.registered.push((rect, topic));
        }
    }

    /// Register a fallback topic underneath the current entries. Used for
    /// whole panels so any control that is not part of a specific section
    /// still gets contextual help, while the finer section rectangles win.
    pub fn register_front(&mut self, rect: Rect, topic: HelpTopic) {
        if rect.is_positive() {
            self.registered.insert(0, (rect, topic));
        }
    }

    pub fn last_rect(&self) -> Rect {
        self.last_rect
    }

    pub fn resolve(&mut self, ctx: &egui::Context) {
        let Some(pointer) = ctx.input(|input| input.pointer.hover_pos()) else {
            self.hovered = None;
            if !self.pinned {
                self.shown = None;
            }
            return;
        };
        if self.last_rect.is_positive() && self.last_rect.contains(pointer) {
            return;
        }
        let mut found = None;
        for (rect, topic) in self.registered.iter().rev() {
            if rect.contains(pointer) {
                found = Some(*topic);
                break;
            }
        }
        self.hovered = found;
        if let Some(topic) = found {
            self.shown = Some(topic);
        } else if !self.pinned {
            self.shown = None;
        }
    }

    /// Draw the card and return its rectangle (used to keep the viewport
    /// editor from adding keys when clicking the help window).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        context: &HelpContext<'_>,
        available: &[HelpTopic],
        bottom_offset: f32,
    ) -> Rect {
        if !self.enabled || viewport.width() < 300.0 || viewport.height() < 260.0 {
            self.last_rect = Rect::NOTHING;
            return self.last_rect;
        }
        let Some(topic) = self.shown else {
            self.last_rect = Rect::NOTHING;
            return self.last_rect;
        };
        let width = (viewport.width() * 0.46).clamp(360.0, 540.0);
        let max_height = (viewport.height() - bottom_offset - 26.0).max(240.0);
        let pos = Pos2::new(
            viewport.left() + 12.0,
            viewport.bottom() - 12.0 - bottom_offset,
        );
        let mut close = false;
        let mut jump = None;
        let area = egui::Area::new(egui::Id::unique("vf-context-help"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(15, 20, 28, 248))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(80, 106, 138)))
                    .corner_radius(9.0)
                    .inner_margin(egui::Margin::same(11))
                    .show(ui, |ui| {
                        ui.set_max_width(width);
                        ui.set_max_height(max_height + 40.0);
                        // Header: section, title, navigation and pin.
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(topic.section().to_uppercase())
                                        .size(8.5)
                                        .color(Color32::from_rgb(120, 156, 192)),
                                );
                                ui.label(
                                    egui::RichText::new(topic.title())
                                        .strong()
                                        .size(14.0)
                                        .color(Color32::from_rgb(226, 240, 255)),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("×").on_hover_text("Fermer (F1)").clicked()
                                    {
                                        close = true;
                                    }
                                    if ui
                                        .selectable_label(self.pinned, "Épingler")
                                        .on_hover_text("Épingler cette fiche")
                                        .clicked()
                                    {
                                        self.pinned = !self.pinned;
                                    }
                                    let list = if available.is_empty() {
                                        &HelpTopic::ALL[..]
                                    } else {
                                        available
                                    };
                                    if ui
                                        .small_button("›")
                                        .on_hover_text("Fiche suivante")
                                        .clicked()
                                        && let Some(index) =
                                            list.iter().position(|candidate| *candidate == topic)
                                    {
                                        jump = Some(list[(index + 1) % list.len()]);
                                    }
                                    if ui
                                        .small_button("‹")
                                        .on_hover_text("Fiche précédente")
                                        .clicked()
                                    {
                                        let index = list
                                            .iter()
                                            .position(|candidate| *candidate == topic)
                                            .unwrap_or(0);
                                        jump = Some(list[(index + list.len() - 1) % list.len()]);
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                        // Table of contents.
                        ui.horizontal(|ui| {
                            let mut selected = None;
                            egui::ComboBox::from_id_salt("vf-help-toc")
                                .selected_text("Sommaire des tutos")
                                .width(168.0)
                                .show_ui(ui, |ui| {
                                    let list = if available.is_empty() {
                                        &HelpTopic::ALL[..]
                                    } else {
                                        available
                                    };
                                    egui::ScrollArea::vertical()
                                        .max_height(360.0)
                                        .show(ui, |ui| {
                                            let mut section = "";
                                            for candidate in list {
                                                if candidate.section() != section {
                                                    section = candidate.section();
                                                    ui.add_space(3.0);
                                                    ui.label(
                                                        egui::RichText::new(section.to_uppercase())
                                                            .size(9.0)
                                                            .color(Color32::from_rgb(
                                                                130, 156, 188,
                                                            )),
                                                    );
                                                }
                                                if ui
                                                    .selectable_label(
                                                        *candidate == topic,
                                                        candidate.title(),
                                                    )
                                                    .clicked()
                                                {
                                                    selected = Some(*candidate);
                                                }
                                            }
                                        });
                                });
                            if let Some(selected) = selected {
                                jump = Some(selected);
                            }
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(max_height)
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(topic.summary())
                                        .size(11.0)
                                        .color(Color32::from_rgb(205, 218, 234)),
                                );
                                if let Some(line) = topic.live_line(context) {
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new(line)
                                            .monospace()
                                            .size(9.5)
                                            .color(Color32::from_rgb(120, 200, 220)),
                                    );
                                }
                                ui.add_space(5.0);
                                for bullet in topic.bullets() {
                                    ui.label(
                                        egui::RichText::new(format!("• {bullet}"))
                                            .size(10.2)
                                            .color(Color32::from_rgb(184, 198, 216)),
                                    );
                                }
                                ui.add_space(6.0);
                                if let Some(image) = topic.image()
                                    && let Some(texture) = self.texture(ctx, image)
                                {
                                    let size = texture.size_vec2();
                                    let scale = (width / size.x).min(1.0);
                                    ui.add(
                                        egui::Image::from_texture(egui::load::SizedTexture::new(
                                            texture.id(),
                                            size * scale,
                                        ))
                                        .corner_radius(5.0),
                                    );
                                    ui.label(
                                        egui::RichText::new(topic.caption())
                                            .italics()
                                            .size(9.5)
                                            .color(Color32::from_rgb(168, 184, 202)),
                                    );
                                }
                                let try_it = topic.try_it();
                                if !try_it.is_empty() {
                                    ui.add_space(5.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "À essayer : {}",
                                            try_it.trim_start_matches("À essayer : ")
                                        ))
                                        .size(9.5)
                                        .color(Color32::from_rgb(255, 214, 138)),
                                    );
                                }
                            });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(
                                "F1 : ouvrir/fermer · survolez un réglage · Épingler fige la fiche",
                            )
                            .size(8.5)
                            .color(Color32::from_rgb(112, 130, 152)),
                        );
                    });
            });
        self.last_rect = area.response.rect;
        if let Some(topic) = jump {
            self.shown = Some(topic);
            self.pinned = true;
        }
        if close {
            self.close();
        }
        self.last_rect
    }

    fn texture(&mut self, ctx: &egui::Context, image: HelpImage) -> Option<&egui::TextureHandle> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.textures.entry(image) {
            let decoded = image::load_from_memory(image.bytes()).ok()?.to_rgba8();
            let size = [decoded.width() as usize, decoded.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
            let texture = ctx.load_texture(
                format!("help-{image:?}"),
                color,
                egui::TextureOptions::LINEAR,
            );
            entry.insert(texture);
        }
        self.textures.get(&image)
    }
}

fn target_mode_label(mode: TargetMode) -> &'static str {
    match mode {
        TargetMode::Fixed => "Fixe",
        TargetMode::Path => "Tracé animé",
        TargetMode::EyeAhead => "Avance",
        TargetMode::Offset => "Décalage",
        TargetMode::Trail => "Retard",
        TargetMode::Manual => "Manuel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_complete_card() {
        let mut seen = std::collections::HashSet::new();
        for topic in HelpTopic::ALL {
            assert!(seen.insert(topic), "duplicate topic {topic:?}");
            assert!(!topic.title().is_empty());
            assert!(!topic.section().is_empty());
            assert!(topic.summary().len() > 40, "{topic:?}");
            assert!(topic.bullets().len() >= 3, "{topic:?}");
            assert!(!topic.caption().is_empty());
            assert!(topic.image().is_some(), "{topic:?} has no visual");
        }
    }

    #[test]
    fn every_help_image_is_a_valid_16_9_png() {
        let mut images = std::collections::HashSet::new();
        for topic in HelpTopic::ALL {
            if let Some(image) = topic.image() {
                images.insert(image);
            }
        }
        for image in images {
            let bytes = image.bytes();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{image:?}");
            let decoded = image::load_from_memory(bytes)
                .unwrap_or_else(|error| panic!("{image:?}: {error}"))
                .to_rgba8();
            assert_eq!(decoded.dimensions(), (960, 540), "{image:?}");
        }
    }

    #[test]
    fn hover_resolution_uses_the_most_specific_rectangle() {
        let ctx = egui::Context::default();
        let mut help = HelpState::new();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let pointer = Pos2::new(200.0, 300.0);
        // The panel fallback is wide; the section is nested inside it.
        let panel = Rect::from_min_max(Pos2::new(0.0, 100.0), Pos2::new(360.0, 600.0));
        let section = Rect::from_min_max(Pos2::new(0.0, 200.0), Pos2::new(360.0, 420.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(1.0),
            events: vec![egui::Event::PointerMoved(pointer)],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            help.begin_frame();
            help.register_front(panel, HelpTopic::Timing);
            help.register(section, HelpTopic::Optics);
            help.resolve(ui.ctx());
        });
        output.textures_delta.clear();
        assert_eq!(help.hovered, Some(HelpTopic::Optics));
        assert_eq!(help.shown, Some(HelpTopic::Optics));
        assert!(help.is_open());
    }

    #[test]
    fn topics_can_be_named_from_the_command_line() {
        for topic in HelpTopic::ALL {
            let debug = format!("{topic:?}").to_ascii_lowercase();
            assert_eq!(HelpTopic::parse(&debug), Some(topic));
            assert_eq!(HelpTopic::parse(topic.title()), Some(topic));
        }
        assert_eq!(HelpTopic::parse("does not exist"), None);
    }
}
