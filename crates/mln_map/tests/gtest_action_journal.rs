// Ports test/util/action_journal.test.cpp: placeholders. util::ActionJournal
// (src/mln/util/action_journal*.cpp: the map's event log written to rolling
// files, `Map(..., ActionJournalOptions)`, `Map::getActionJournal()`) is not
// ported (`Map::with_resource_options` has a TODO(port) for its options).

// gtest: ActionJournal.Create
#[test]
#[ignore = "TODO(port): util::ActionJournal (ActionJournalOptions, Map::getActionJournal)"]
fn create() {
    // test/util/action_journal.test.cpp:77: creating a map with an enabled
    // journal logs one `onMapCreate` event into one file in the journal's
    // log directory.
}

// gtest: ActionJournal.Clear
#[test]
#[ignore = "TODO(port): util::ActionJournal (ActionJournalOptions, Map::getActionJournal)"]
fn clear() {
    // test/util/action_journal.test.cpp:92: clearLog() keeps the directory
    // and the (empty) log file.
}

// gtest: ActionJournal.ReadPreviousSession
#[test]
#[ignore = "TODO(port): util::ActionJournal (ActionJournalOptions, Map::getActionJournal)"]
fn read_previous_session() {
    // test/util/action_journal.test.cpp:103: create, destroy and re-create a
    // map: the log reads onMapCreate, onMapDestroy, onMapCreate.
}

// gtest: ActionJournal.RollFiles
#[test]
#[ignore = "TODO(port): util::ActionJournal (ActionJournalOptions, Map::getActionJournal)"]
fn roll_files() {
    // test/util/action_journal.test.cpp:120: with withLogFileSize(1) and
    // withLogFileCount(2), loading test/fixtures/api/empty.json and
    // rendering rolls the files: 2 files, 2 log entries, the newest file
    // holds onMapDestroy after the map goes away.
}

// gtest: ActionJournal.ValidateEvents
#[test]
#[ignore = "TODO(port): util::ActionJournal (ActionJournalOptions, Map::getActionJournal)"]
fn validate_events() {
    // test/util/action_journal.test.cpp:211: every MapObserver /
    // RendererObserver event the journal records, with its JSON fields.
}

// gtest: ActionJournal.RenderingStats
#[test]
#[ignore = "TODO(port): util::ActionJournal (its renderingStats event)"]
fn rendering_stats() {
    // test/util/action_journal.test.cpp:432: onDidFinishRenderingFrame stats
    // are aggregated per withRenderingStatsReportInterval(2).
}
