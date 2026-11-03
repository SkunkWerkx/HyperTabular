<?php

declare(strict_types=1);

foreach (['hypercast', 'hypertabular'] as $extension) {
    if (!extension_loaded($extension)) {
        fwrite(STDERR, "the {$extension} extension is not loaded; run composer test" . PHP_EOL);
        exit(1);
    }
}

require __DIR__ . '/../vendor/autoload.php';
