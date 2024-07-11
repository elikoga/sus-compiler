use std::{cell::RefCell, ops::Range};

use crate::{file_position::FileText, pretty_print_spans_in_reverse_order};

/// Many duplicates will be produced, and filtering them out in the code itself is inefficient. Therefore just keep a big buffer and deduplicate as needed
const SPAN_TOUCH_HISTORY_SIZE: usize = 256;
const NUM_SPANS_TO_PRINT: usize = 10;
const DEFAULT_RANGE: Range<usize> = usize::MAX..usize::MAX;

/// Register a [crate::file_position::Span] for potential printing by [PanicGuardSpanPrinter] on panic.
///
/// Would like to use [crate::file_position::Span], but cannot copy the span because that would create infinite loop.
///
/// So use [Range] instead.
pub fn add_debug_span(span_rng: Range<usize>) {
    // Convert to range so we don't invoke any of Span's triggers
    SPANS_HISTORY.with_borrow_mut(|history| {
        let cur_idx = history.num_spans;
        history.num_spans += 1;

        history.span_history[cur_idx % SPAN_TOUCH_HISTORY_SIZE] = span_rng;
    });
}

struct TouchedSpansHistory {
    span_history: [Range<usize>; SPAN_TOUCH_HISTORY_SIZE],
    num_spans: usize,
    in_use: bool,
}

thread_local! {
    static SPANS_HISTORY : RefCell<TouchedSpansHistory> =
        RefCell::new(TouchedSpansHistory{
            span_history : [DEFAULT_RANGE; SPAN_TOUCH_HISTORY_SIZE],
            num_spans : 0,
            in_use : false
        });
}

fn print_most_recent_spans(file_text: String) {
    let spans_to_print: Vec<Range<usize>> = SPANS_HISTORY.with_borrow_mut(|history| {
        assert!(history.in_use);

        let mut spans_to_print: Vec<Range<usize>> = Vec::with_capacity(NUM_SPANS_TO_PRINT);

        let end_at = if history.num_spans > SPAN_TOUCH_HISTORY_SIZE {
            history.num_spans - SPAN_TOUCH_HISTORY_SIZE
        } else {
            0
        };

        let mut cur_i = history.num_spans;
        while cur_i > end_at {
            cur_i -= 1;
            let grabbed_span = history.span_history[cur_i % SPAN_TOUCH_HISTORY_SIZE].clone();
            if !spans_to_print.contains(&grabbed_span) {
                spans_to_print.push(grabbed_span);
            }
            if spans_to_print.len() >= NUM_SPANS_TO_PRINT {
                break;
            }
        }

        spans_to_print
    });

    println!("Panic unwinding. Printing the last {} spans. BEWARE: These spans may not correspond to this file, thus incorrect spans are possible!", spans_to_print.len());
    pretty_print_spans_in_reverse_order(file_text, spans_to_print);
}

/// Print the last [NUM_SPANS_TO_PRINT] touched spans on panic to aid in debugging
///
/// If not defused, it will print when dropped, ostensibly when being unwound from a panic
///
/// Must call [Self::defuse] when no panic occurred
///
/// This struct uses a shared thread_local resource: [SPANS_HISTORY], so no two can exist at the same time (within the same thread).
///
/// Maybe future work can remove dependency on Linker lifetime with some unsafe code.
pub struct SpanDebugger<'text> {
    context: &'text str,
    file_text: &'text FileText,
    defused: bool,
}

impl<'text> SpanDebugger<'text> {
    pub fn new(context: &'text str, file_text: &'text FileText) -> Self {
        SPANS_HISTORY.with_borrow_mut(|history| {
            assert!(!history.in_use);
            history.in_use = true;
            history.num_spans = 0;
        });

        Self {
            context,
            file_text,
            defused: false,
        }
    }

    pub fn defuse(&mut self) {
        SPANS_HISTORY.with_borrow_mut(|history| {
            assert!(history.in_use);
            history.in_use = false;
        });

        self.defused = true;
    }
}

impl<'text> Drop for SpanDebugger<'text> {
    fn drop(&mut self) {
        if !self.defused {
            println!("Panic happened in Span-guarded context: {}", self.context);
            print_most_recent_spans(self.file_text.file_text.clone())
        }
    }
}
