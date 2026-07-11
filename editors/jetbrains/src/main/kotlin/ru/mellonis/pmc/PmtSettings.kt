package ru.mellonis.pmc

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-level persisted settings: the `pmt` binary path and the
 * lint code allow-list, backed by `pmt.xml` in the IDE config directory.
 *
 * External shape preserved from the Task 4 stub: `PmtSettings.instance.state`
 * with `pmtPath` / `lintAllow` (defaults `"pmt"` / empty) —
 * [PmtLanguageServerFactory] and [PmtVersionCheck] read through this shape
 * unchanged now that it is a real [PersistentStateComponent].
 */
@State(name = "PmtSettings", storages = [Storage("pmt.xml")])
class PmtSettings : PersistentStateComponent<PmtSettings.State> {
    data class State(
        var pmtPath: String = "pmt",
        var lintAllow: MutableList<String> = mutableListOf(),
    )

    companion object {
        val instance: PmtSettings get() = service()
    }

    private var myState = State()

    override fun getState(): State = myState

    override fun loadState(state: State) {
        myState = state
    }
}
