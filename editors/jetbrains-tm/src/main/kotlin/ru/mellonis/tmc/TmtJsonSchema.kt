package ru.mellonis.tmc

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.jetbrains.jsonSchema.extension.JsonSchemaFileProvider
import com.jetbrains.jsonSchema.extension.JsonSchemaProviderFactory
import com.jetbrains.jsonSchema.extension.SchemaType
import com.jetbrains.jsonSchema.impl.JsonSchemaVersion

/**
 * Attaches the bundled `tmt.schema.json` (copied into the plugin's
 * `schemas/` resources at build time from `editors/schemas/`) to every
 * file named `tmt.json`, giving the manifest validation and completion
 * without the manual Settings mapping. Registered behind an optional
 * dependency on the JSON plugin, so IDEs without it just skip this.
 */
class TmtJsonSchemaProviderFactory : JsonSchemaProviderFactory {
    override fun getProviders(project: Project): List<JsonSchemaFileProvider> =
        listOf(TmtJsonSchemaFileProvider())
}

class TmtJsonSchemaFileProvider : JsonSchemaFileProvider {
    override fun isAvailable(file: VirtualFile): Boolean = file.name == "tmt.json"

    override fun getName(): String = "tmt project manifest"

    override fun getSchemaFile(): VirtualFile? =
        JsonSchemaProviderFactory.getResourceFile(
            TmtJsonSchemaFileProvider::class.java, "/schemas/tmt.schema.json")

    override fun getSchemaType(): SchemaType = SchemaType.embeddedSchema

    override fun getSchemaVersion(): JsonSchemaVersion = JsonSchemaVersion.SCHEMA_7
}
