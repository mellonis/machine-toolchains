package ru.mellonis.pmc

// Task 5 replaces this stub with a real application-level
// PersistentStateComponent (binary path + lint allow-list, backed by
// pmt.xml). This placeholder exists only so PmtLanguageServerFactory and
// PmtVersionCheck compile ahead of that task; Task 5's implementation must
// preserve the external shape used below (`PmtSettings.instance.state`
// with `pmtPath` / `lintAllow` fields, defaults `"pmt"` / empty).
object PmtSettings {
    class State {
        var pmtPath: String = "pmt"
        var lintAllow: MutableList<String> = mutableListOf()
    }

    val instance: PmtSettings get() = this
    val state: State = State()
}
