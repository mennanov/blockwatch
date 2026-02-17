function validate(ctx, content)
  local expected = ctx.attrs["expected"]
  if content ~= expected then
    return "expected '" .. expected .. "' but got '" .. content .. "'"
  end
  return nil
end
