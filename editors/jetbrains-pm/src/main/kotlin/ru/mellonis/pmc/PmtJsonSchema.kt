package ru.mellonis.pmc

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.jetbrains.jsonSchema.extension.JsonSchemaFileProvider
import com.jetbrains.jsonSchema.extension.JsonSchemaProviderFactory
import com.jetbrains.jsonSchema.extension.SchemaType
import com.jetbrains.jsonSchema.impl.JsonSchemaVersion

/**
 * Attaches the bundled `pmt.schema.json` (copied into the plugin's
 * `schemas/` resources at build time from `editors/schemas/`) to every
 * file named `pmt.json`, giving the manifest validation and completion
 * without the manual Settings mapping. Registered behind an optional
 * dependency on the JSON plugin, so IDEs without it just skip this.
 */
class PmtJsonSchemaProviderFactory : JsonSchemaProviderFactory {
    override fun getProviders(project: Project): List<JsonSchemaFileProvider> =
        listOf(PmtJsonSchemaFileProvider())
}

class PmtJsonSchemaFileProvider : JsonSchemaFileProvider {
    override fun isAvailable(file: VirtualFile): Boolean = file.name == "pmt.json"

    override fun getName(): String = "pmt project manifest"

    override fun getSchemaFile(): VirtualFile? =
        JsonSchemaProviderFactory.getResourceFile(
            PmtJsonSchemaFileProvider::class.java, "/schemas/pmt.schema.json")

    override fun getSchemaType(): SchemaType = SchemaType.embeddedSchema

    override fun getSchemaVersion(): JsonSchemaVersion = JsonSchemaVersion.SCHEMA_7
}
