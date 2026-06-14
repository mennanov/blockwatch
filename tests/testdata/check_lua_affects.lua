-- Verifies, without any IO, that every block referenced via `affects` stays in
-- sync with the validated block's content. The affected blocks are provided in
-- `ctx.affects` as a list of { file, name, content } tables.
function validate(ctx, content)
  if ctx.affects == nil then
    return "ctx.affects is nil"
  end
  for _, affected in ipairs(ctx.affects) do
    if affected.content ~= content then
      return "block '" .. affected.name .. "' in " .. affected.file .. " is out of sync"
    end
  end
  return nil
end
