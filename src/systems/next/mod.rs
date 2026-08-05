//! Échafaudage pour un futur système NeXT — **pas un système fonctionnel**.
//!
//! Contrairement à `systems::atari_st`, qui a pu s'appuyer tout du long sur
//! Hatari/Steem SSE (sources C locales, cycle-exactes, utilisées comme
//! référence de correction à chaque étape), aucune ROM NeXT ni émulateur de
//! référence (par ex. Previous) n'est disponible sur cette machine au
//! moment d'écrire ce module. Construire un `Bus` branché sur une carte
//! mère NeXT réelle (mapping mémoire, PMMU 68030/68040, SCSI, Ethernet,
//! son, lecteur MO...) sans rien contre quoi vérifier serait un exercice de
//! devinette, pas de l'émulation — ce module se limite donc à la structure,
//! en attendant une session dédiée avec ces références en main.
//!
//! Ce qui existe déjà côté cœur CPU et que ce système consommera : le
//! sous-ensemble 68010 (`CpuType::M68010`, voir `crate::cpu`) — première
//! étape vers 68020/68030/68040, qu'un NeXT réel demande selon le modèle
//! (voir [`model::NextModel`]).

pub mod model;

use crate::Cpu;

/// État d'un système NeXT — pour l'instant, seulement le cœur CPU. Pas
/// d'implémentation [`crate::Bus`], pas de RAM/ROM, pas de chargement
/// d'image disque : voir la doc du module.
pub struct Next {
    pub cpu: Cpu,
}

impl Next {
    pub fn new(model: model::NextModel) -> Self {
        let mut cpu = Cpu::new();
        cpu.cpu_type = model.cpu_type();
        Next { cpu }
    }
}
