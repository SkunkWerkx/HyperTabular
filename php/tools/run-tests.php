<?php

declare(strict_types=1);

// `composer test`: runs PHPUnit in a PHP that has both extensions loaded (tools/extension.php
// says which). No shell substitution, so it works on Windows too.

require __DIR__ . '/extension.php';

$root = \dirname(__DIR__);
$flags = [];
foreach (hypertabular_extensions($root) as $path) {
    $flags[] = '-d';
    $flags[] = 'extension=' . $path;
}
hypertabular_run([PHP_BINARY, ...$flags, $root . '/vendor/bin/phpunit', ...\array_slice($argv, 1)], $root, 'phpunit');
