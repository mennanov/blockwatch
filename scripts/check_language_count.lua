-- Checks that the number of languages the README's pitch claims is the number of items in its list of supported
-- languages. The list's block points at the pitch with `affects`, so the pitch arrives in `ctx.affects`, and the script
-- needs no file access. `check-lua-pattern` passes one value for each item, so an item that wraps over two lines
-- counts once.

function validate(ctx, content)
    local pitch = ctx.affects and ctx.affects[1]
    if not pitch then
        return "the block must point at the pitch with affects"
    end
    local claimed = pitch.content:match("(%d+) languages")
    if not claimed then
        return "the pitch does not say how many languages are supported"
    end
    if tonumber(claimed) ~= #content then
        return string.format("the pitch claims %s languages, but the list has %d", claimed, #content)
    end
    return nil
end
