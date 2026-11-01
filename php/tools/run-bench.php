<?php

declare(strict_types=1);

// `composer bench`: runs PHPBench with both extensions loaded (tools/extension.php says which).
// PHPBench times each iteration in a PHP process of its own, which takes no -d flag from this
// one, so the extensions reach those processes through --php-config, one `extension` setting
// each. Arguments pass through: `composer bench -- --report=aggregate`.

require __DIR__ . '/extension.php';

$root = \dirname(__DIR__);
$config = json_encode(['extension' => hypertabular_extensions($root)], JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES);
hypertabular_run(
    [PHP_BINARY, $root . '/vendor/bin/phpbench', 'run', '--php-config=' . $config, ...\array_slice($argv, 1)],
    $root,
    'phpbench',
);
