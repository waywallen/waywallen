local target = lito.target({ kind = "lib", name = "waywallen-switch" })
local schema = lito.external_source(target, "waywallen-control")
local generated = lito.run({
  tool = lito.tool("protoc"),
  tools = { lito.host_tool(target, "liproto-codegen", "protoc-gen-liproto") },
  cwd = ".",
  input_roots = { schema },
  inputs = { schema.file("control.proto"), schema.file("filter.proto") },
  args = {
    "-I@INPUT_ROOT:1@",
    "--plugin=protoc-gen-liproto=@TOOL:1@",
    "--liproto_out=split:@OUTPUT_DIR@",
    "@INPUT:1@", "@INPUT:2@",
  },
  outputs = { "control.cppm", "control.cpp", "filter.cppm", "filter.cpp" },
})
for _, source in ipairs(generated.outputs) do
  lito.target_add_generated_source(target, source)
end
