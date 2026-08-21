function validate(ctx, content)
  if os == nil then
    return "os library is not available"
  end
  local t = os.clock()
  if t == nil then
    return "os.clock() returned nil"
  end
  return nil
end
