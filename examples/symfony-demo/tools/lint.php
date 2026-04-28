<?php

declare(strict_types=1);

$roots = ['bin', 'public', 'src', 'tests', 'tools'];
$failed = false;

foreach ($roots as $root) {
    $directory = dirname(__DIR__) . DIRECTORY_SEPARATOR . $root;
    $iterator = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($directory));

    foreach ($iterator as $file) {
        if (!$file->isFile()) {
            continue;
        }

        $path = $file->getPathname();
        if ($file->getExtension() !== 'php' && basename($path) !== 'console') {
            continue;
        }

        $command = escapeshellarg(PHP_BINARY) . ' -l ' . escapeshellarg($path);
        passthru($command, $exitCode);
        $failed = $failed || $exitCode !== 0;
    }
}

exit($failed ? 1 : 0);
