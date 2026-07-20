package ru.mellonis.tmc

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-level persisted settings: the `tmt` binary path, the lint
 * code allow-list, and the opt-in lint warn-list, backed by `tmt.xml` in
 * the IDE config directory.
 *
 * `lintWarn` has no `tmt.json` counterpart — the project file carries
 * `lint.allow` only — so enabling an opt-in rule (the totality lint) is an
 * IDE-side choice, exactly as it is a `--warn` flag choice on the command
 * line.
 */
@State(name = "TmtSettings", storages = [Storage("tmt.xml")])
class TmtSettings : PersistentStateComponent<TmtSettings.State> {
    data class State(
        var tmtPath: String = "tmt",
        var lintAllow: MutableList<String> = mutableListOf(),
        var lintWarn: MutableList<String> = mutableListOf(),
    )

    companion object {
        val instance: TmtSettings get() = service()
    }

    private var myState = State()

    override fun getState(): State = myState

    override fun loadState(state: State) {
        myState = state
    }
}
