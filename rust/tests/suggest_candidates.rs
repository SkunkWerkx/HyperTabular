//! What an ambiguous answer offers, read straight from the core's words: the core says text for every
//! `DateTime` column today, so no sample reaches this door.

use crate::kernel::abi::RawSuggestion;
use crate::suggest::{Candidate, SuggestStatus, read_status};
use crate::{DateOrder, Door};

#[test]
fn an_ambiguous_date_offers_orders_whichever_date_door_it_names() {
    for door in [
        Door::DateOrdered(DateOrder::MonthDayYear),
        Door::DateTime(DateOrder::MonthDayYear),
    ] {
        let found = RawSuggestion {
            door: door.code(),
            param: 1 << 2 | 1 << 3,
            status: RawSuggestion::AMBIGUOUS,
            matched: 1,
            sampled: 1,
        };
        assert_eq!(
            read_status(&found),
            (
                SuggestStatus::Ambiguous,
                vec![
                    Candidate::DateOrder(DateOrder::MonthDayYear),
                    Candidate::DateOrder(DateOrder::DayMonthYear),
                ]
            ),
            "{door:?}"
        );
    }
    // A number's are notations, under the same bits.
    let found = RawSuggestion {
        door: Door::I32.code(),
        param: RawSuggestion::NOTATION_COMMA_GROUPS | RawSuggestion::NOTATION_COMMA_DECIMAL,
        status: RawSuggestion::AMBIGUOUS,
        matched: 1,
        sampled: 1,
    };
    let (_, candidates) = read_status(&found);
    assert!(
        candidates
            .iter()
            .all(|one| matches!(one, Candidate::NumFormat(_)))
    );
    assert_eq!(candidates.len(), 2);
}
