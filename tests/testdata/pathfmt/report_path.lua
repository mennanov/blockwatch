-- Reports the path the validator was given, so a test can compare it across run modes.
function validate(ctx, content)
  return "ctx.file=" .. ctx.file
end
