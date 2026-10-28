//! Interactive fuzzy picker. The UI is drawn on stderr (inquire), so stdout
//! stays free for the selected path.

use std::fmt;
use std::io::IsTerminal;

use eyre::{Result, bail};
use inquire::Select;
use inquire::error::InquireError;
use inquire::ui::{RenderConfig, Styled};

use crate::entries::Entry;
use crate::fuzzy;

struct Item(Entry);

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.label)?;
        if !self.0.tags.is_empty() {
            write!(f, "  [{}]", self.0.tags.join(", "))?;
        }
        Ok(())
    }
}

/// `Ok(None)` when the user cancels.
pub fn pick(entries: Vec<Entry>, query: Option<&str>) -> Result<Option<Entry>> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("pick needs an interactive terminal; use `resolve` or `list --json` instead");
    }
    let items: Vec<Item> = entries.into_iter().map(Item).collect();
    let scorer =
        |input: &str, item: &Item, _: &str, _: usize| fuzzy::score(input, &item.0.search_text());
    let prompt = Select::new("Select a project:", items)
        .with_starting_filter_input(query.unwrap_or_default())
        .with_scorer(&scorer)
        .with_vim_mode(false)
        .with_render_config(
            RenderConfig::default_colored()
                .with_scroll_up_prefix(Styled::new("↑"))
                .with_scroll_down_prefix(Styled::new("↓")),
        )
        .prompt();
    match prompt {
        Ok(item) => Ok(Some(item.0)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
