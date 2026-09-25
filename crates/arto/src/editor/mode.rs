//! How the document is shown while it is being edited.

/// The two ways to edit a document. Both edit the same Markdown source, and
/// switching between them changes nothing about it — only how it is drawn,
/// and whether the rendered page stands beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorMode {
    /// The page edited in place: the source drawn as what it means, markup
    /// shown only where the caret is, across the whole width. For fixing and
    /// rewording what is already there.
    #[default]
    Rich,
    /// Every character as written, beside the rendered page. For the parts
    /// the live preview leaves as source — tables, diagrams, formulas — and
    /// for reshaping the document.
    Source,
}

impl EditorMode {
    /// The other one.
    pub fn switched(self) -> Self {
        match self {
            Self::Rich => Self::Source,
            Self::Source => Self::Rich,
        }
    }

    /// The name the frontend and the stylesheet know it by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rich => "rich",
            Self::Source => "source",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rich => "Rich",
            Self::Source => "Source",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_twice_is_where_it_started() {
        for mode in [EditorMode::Rich, EditorMode::Source] {
            assert_ne!(mode.switched(), mode);
            assert_eq!(mode.switched().switched(), mode);
        }
    }

    #[test]
    fn editing_starts_rich() {
        assert_eq!(EditorMode::default(), EditorMode::Rich);
    }
}
