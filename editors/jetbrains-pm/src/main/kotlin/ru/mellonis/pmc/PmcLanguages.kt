package ru.mellonis.pmc

import com.intellij.lang.Language

/**
 * Marker languages backing the PMC/PMA file types. No parser definition
 * is registered for either — PSI falls back to plain text, which is all
 * the LSP-driven features need — but a LANGUAGE file type is what routes
 * the editor-highlighter construction through the file-type provider
 * lookup on newer platforms (a plain FileType's editor bypasses it and
 * loses TextMate token data in the default highlighter storage).
 */
object PmcLanguage : Language("pmc")

object PmaLanguage : Language("pma")
