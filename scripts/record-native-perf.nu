#!/usr/bin/env nu
# Run serially, with no builds or other benchmarks during capture.

const ROOT = path self ..
const DEFAULT_BINARY = ($ROOT | path join target release carnet)
const METAL_TABLES = '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals" or @schema="displayed-surfaces-interval" or @schema="display-vsyncs-interval"]'

# Record and analyze a native Carnet benchmark with Instruments.
def main [
    note: path
    output: path
    mode: string = "window" # page, window, or scroll
    seconds: number = 10 # Per workload
    binary: path = $DEFAULT_BINARY
] {
    let output = ($output | path expand --no-symlink)
    let trace = $"($output).trace"
    let metadata = ($output | path join metadata.json)
    if ($trace | path exists) or ($output | path exists) {
        print --stderr $"Output already exists: ($output)"
        exit 2
    }

    let note = ($note | path expand --strict)
    let binary = ($binary | path expand --strict)
    mkdir ($output | path dirname)
    ^mkdir -- $output
    let limit = $"(2 * $seconds + 10)s"
    mut options = if $env.CARNET_GPU_DISPLAY_ONLY? == "1" {
        [record --instrument GPU --instrument Display]
    } else if $env.CARNET_PRESENTATION_ONLY? == "1" {
        [record --instrument Display]
    } else {
        [record --template ($env.CARNET_METAL_TEMPLATE? | default --empty "Metal System Trace")]
    }
    for name in [KUI_FRAME_LATENCY KUI_FRAME_PACING KUI_WINDOW_AT KUI_SPECIALIZE KUI_ACQUIRE_THREAD CARNET_PERF_WINDOW] {
        let value = ($env | get -o $name)
        if $value != null {
            $options = ($options | append [--env $"($name)=($value)"])
        }
    }

    {
        binary: $binary
        binary_sha256: (open --raw $binary | hash sha256)
        source_sha256: (open --raw $note | hash sha256)
        xctrace_options: $options
    } | to json --indent 2 | save ($output | path join recording.json)

    ^system_profiler SPDisplaysDataType out> ($output | path join displays.txt)
    ^pmset -g batt out> ($output | path join power.txt)
    let app_log = ($output | path join app.log)
    let trace_log = ($output | path join xctrace.log)
    (^xcrun xctrace ...$options
        --output $trace --time-limit $limit --target-stdout $app_log
        --launch -- $binary --native-perf $note $output $mode $seconds
        out+err> $trace_log)
    if not ($metadata | path exists) {
        if ($app_log | path exists) { open --raw $app_log | print --stderr }
        error make {msg: $"Carnet did not write ($metadata). See ($trace_log)."}
    }
    ^xcrun xctrace export --input $trace --toc --output ($output | path join metal-toc.xml)
    ^xcrun xctrace export --input $trace --xpath $METAL_TABLES --output ($output | path join metal-data.xml)
    ^python3 ($ROOT | path join scripts analyze-native-perf.py) $output --single-case
}
