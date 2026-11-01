<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

/**
 * The `HyperTabular\` API as reflection sees it, declaration for declaration: what StubTest
 * holds the committed stub to.
 */
final class Surface
{
    /** @return array<string, array<string, mixed>> */
    public static function of(): array
    {
        $names = array_filter(
            [...get_declared_classes(), ...get_declared_interfaces()],
            static fn (string $name): bool => str_starts_with($name, 'HyperTabular\\')
                && !str_starts_with($name, 'HyperTabular\\Tests\\')
                && !str_starts_with($name, 'HyperTabular\\Bench\\'),
        );
        sort($names);
        $surface = [];
        foreach ($names as $name) {
            $rc = new \ReflectionClass($name);
            $entry = [
                'kind' => $rc->isEnum() ? 'enum' : ($rc->isInterface() ? 'interface' : 'class'),
                'final' => $rc->isFinal(),
                'readonly' => $rc->isReadOnly(),
                'instantiable' => $rc->isInstantiable(),
                'interfaces' => $rc->getInterfaceNames(),
                'parent' => $rc->getParentClass() ? $rc->getParentClass()->getName() : null,
                'constants' => [],
                'cases' => [],
                'methods' => [],
                'properties' => [],
            ];
            sort($entry['interfaces']);
            foreach ($rc->getReflectionConstants() as $const) {
                if ($const->isEnumCase()) {
                    $case = $const->getValue();
                    $entry['cases'][$const->getName()] = $case instanceof \BackedEnum ? $case->value : null;
                } else {
                    $entry['constants'][$const->getName()] = $const->getValue();
                }
            }
            foreach ($rc->getMethods() as $m) {
                if ($m->getDeclaringClass()->getName() !== $rc->getName() || !$m->isPublic()) {
                    continue;
                }
                $params = [];
                foreach ($m->getParameters() as $p) {
                    $params[] = [
                        'name' => $p->getName(),
                        'type' => $p->getType() === null ? null : (string) $p->getType(),
                        'optional' => $p->isOptional(),
                        'default' => $p->isDefaultValueAvailable() ? var_export($p->getDefaultValue(), true) : null,
                    ];
                }
                $entry['methods'][$m->getName()] = [
                    'static' => $m->isStatic(),
                    'params' => $params,
                    'return' => $m->getReturnType() === null ? null : (string) $m->getReturnType(),
                ];
            }
            ksort($entry['constants']);
            ksort($entry['methods']);
            if (!$rc->isEnum()) {
                foreach ($rc->getProperties(\ReflectionProperty::IS_PUBLIC) as $p) {
                    if ($p->getDeclaringClass()->getName() !== $rc->getName()) {
                        continue;
                    }
                    $entry['properties'][$p->getName()] = [
                        'type' => $p->getType() === null ? null : (string) $p->getType(),
                        'readonly' => $p->isReadOnly(),
                        'static' => $p->isStatic(),
                    ];
                }
                ksort($entry['properties']);
            }
            $surface[$name] = $entry;
        }
        return $surface;
    }
}
