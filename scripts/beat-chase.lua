-- Example Verso script: paste it into Scripts… › New Script.
-- Launches the next column every bar and pumps the master on the bass.

function frame(t)
  verso.set("/master", 0.6 + 0.4 * t.audio.bass)
end

function beat(n)
  if n % 4 == 0 then
    local columns = verso.columns()
    verso.trigger("/column/" .. (n // 4 % columns + 1) .. "/launch")
  end
end
