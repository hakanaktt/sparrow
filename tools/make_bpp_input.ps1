param(
  [string]$Src = 'data/input/swim.json',
  [string]$Dst = 'data/input/swim_bpp.json',
  [string]$Name = 'swim_bpp',
  [int]$Stock = 1000,
  [int]$Cost = 1,
  # Bin side is max(strip_height, max_item_extent) * BinSlack, so every item fits.
  [double]$BinSlack = 1.05
)
$ErrorActionPreference = 'Stop'
$srcAbs = (Resolve-Path $Src).Path
$raw = [System.IO.File]::ReadAllText($srcAbs)
if ($raw -notmatch '"strip_height"\s*:\s*([0-9.eE+-]+)') { throw 'strip_height not found' }
$h = [double]$matches[1]

# Find the largest item extent across all items so the bin is guaranteed to fit them.
# Parse JSON to walk items (cost: one-shot, items list is small).
$json = $raw | ConvertFrom-Json
$maxExtent = 0.0
foreach ($it in $json.items) {
  $shape = $it.shape
  switch ($shape.type) {
    'rectangle' {
      $w = [double]$shape.data.width
      $hi = [double]$shape.data.height
      if ($w -gt $maxExtent) { $maxExtent = $w }
      if ($hi -gt $maxExtent) { $maxExtent = $hi }
    }
    default {
      # simple_polygon | polygon | multi_polygon — bbox from outer coords
      $coords = $null
      if ($shape.type -eq 'simple_polygon') { $coords = $shape.data }
      elseif ($shape.type -eq 'polygon')    { $coords = $shape.data.outer }
      elseif ($shape.type -eq 'multi_polygon') {
        foreach ($p in $shape.data) {
          foreach ($c in $p.outer) {
            $x = [double]$c[0]; $y = [double]$c[1]
            if ([math]::Abs($x) -gt $maxExtent) { $maxExtent = [math]::Abs($x) }
            if ([math]::Abs($y) -gt $maxExtent) { $maxExtent = [math]::Abs($y) }
          }
        }
        continue
      }
      if ($coords) {
        $xs = $coords | ForEach-Object { [double]$_[0] }
        $ys = $coords | ForEach-Object { [double]$_[1] }
        $extent = [math]::Max(($xs | Measure-Object -Maximum).Maximum - ($xs | Measure-Object -Minimum).Minimum,
                              ($ys | Measure-Object -Maximum).Maximum - ($ys | Measure-Object -Minimum).Minimum)
        if ($extent -gt $maxExtent) { $maxExtent = $extent }
      }
    }
  }
}

$binSide = [math]::Max($h, $maxExtent) * $BinSlack
$out = $raw -replace '"name"\s*:\s*"[^"]*"', "`"name`": `"$Name`""
$bin = '"bins": [ { "id": 0, "shape": { "type": "rectangle", "data": { "x_min": 0.0, "y_min": 0.0, "width": ' + $binSide + ', "height": ' + $binSide + ' } }, "stock": ' + $Stock + ', "cost": ' + $Cost + ' } ]'
$out = $out -replace '"strip_height"\s*:\s*[0-9.eE+-]+', $bin
$dstAbs = Join-Path (Get-Location) $Dst
[System.IO.File]::WriteAllText($dstAbs, $out, (New-Object System.Text.UTF8Encoding $false))
Write-Host "wrote $Dst (strip_height=$h, max_item_extent=$maxExtent, slack=$BinSlack -> bin ${binSide}x${binSide}, stock=$Stock)"
