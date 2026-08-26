-- Reports the values `check-lua-pattern` extracted, so a fixture can assert them through the
-- block's `name` attribute.
function validate(ctx, content)
  local expected = ctx.attrs["name"]
  local actual = table.concat(content, ",")
  if actual ~= expected then
    return "expected '" .. expected .. "' but got '" .. actual .. "'"
  end
  return nil
end
