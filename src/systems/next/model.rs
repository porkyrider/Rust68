//! Lexique des modèles NeXT visés à terme — même principe que
//! `systems::atari_st::model` (une machine réelle précise, pas une taille
//! de RAM choisie au hasard), mais ici purement déclaratif pour l'instant :
//! voir la doc de [`super`] pour ce qui manque avant qu'un [`NextModel`]
//! puisse réellement démarrer quoi que ce soit.

/// Un modèle connu de la gamme NeXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextModel {
    /// 1988 : le "Cube" d'origine, 68030 + FPU 68882 + PMMU 68851 externe.
    Cube,
    /// 1990 : boîtier pizza-box, même cœur 68030 que le Cube.
    Station,
    /// 1991 : Cube avec carte "Turbo", 68040 (MMU/FPU intégrés).
    CubeTurbo,
    /// 1991 : Station avec carte "Turbo", 68040.
    StationTurbo,
}

impl NextModel {
    /// Variante de cœur 68k correspondante — seule caractéristique câblée
    /// pour l'instant (voir la doc de [`super`]). `M68010` est un jalon
    /// intermédiaire : ni 68030 (PMMU intégrée) ni 68040 (MMU+FPU intégrés)
    /// ne sont encore implémentés dans [`crate::cpu`], donc aucune variante
    /// ne peut aujourd'hui représenter fidèlement un vrai NeXT — cette
    /// méthode existe pour que le câblage futur (RAM, ROM, ...) ait déjà un
    /// point d'accroche cohérent avec `systems::atari_st::model`.
    pub fn cpu_type(self) -> crate::CpuType {
        crate::CpuType::M68010
    }
}
